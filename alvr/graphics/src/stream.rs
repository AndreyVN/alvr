use super::{GraphicsContext, MAX_PUSH_CONSTANTS_SIZE, staging::StagingRenderer};
use alvr_common::{
    ViewParams,
    glam::{Mat4, UVec2, Vec2, Vec3, Vec4},
};
use alvr_session::{
    FoveatedEncodingConfig, PassthroughMode, UpscalingConfig, foveation_compress_vars,
};
use std::{ffi::c_void, iter, mem, rc::Rc};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingResource, BindingType, Buffer, BufferBindingType,
    BufferDescriptor, BufferUsages, Color, ColorTargetState, ColorWrites, FragmentState, LoadOp,
    PipelineCompilationOptions, PipelineLayoutDescriptor, PrimitiveState, PrimitiveTopology,
    PushConstantRange, RenderPass, RenderPassColorAttachment, RenderPassDescriptor, RenderPipeline,
    RenderPipelineDescriptor, SamplerBindingType, SamplerDescriptor, ShaderModule, ShaderStages,
    StoreOp, TextureSampleType, TextureView, TextureViewDescriptor, TextureViewDimension,
    VertexState, include_wgsl,
};

const FLOAT_SIZE: u32 = mem::size_of::<f32>() as u32;
const U32_SIZE: u32 = mem::size_of::<u32>() as u32;
const VEC4_SIZE: u32 = mem::size_of::<Vec4>() as u32;
const TRANSFORM_SIZE: u32 = mem::size_of::<Mat4>() as u32;

const TRANSFORM_CONST_OFFSET: u32 = 0;
const VIEW_INDEX_CONST_OFFSET: u32 = TRANSFORM_SIZE;
const PASSTHROUGH_MODE_OFFSET: u32 = VIEW_INDEX_CONST_OFFSET + U32_SIZE;
const ALPHA_CONST_OFFSET: u32 = PASSTHROUGH_MODE_OFFSET + U32_SIZE;
const CK_CHANNEL0_CONST_OFFSET: u32 = ALPHA_CONST_OFFSET + FLOAT_SIZE + U32_SIZE;
const CK_CHANNEL1_CONST_OFFSET: u32 = CK_CHANNEL0_CONST_OFFSET + VEC4_SIZE;
const CK_CHANNEL2_CONST_OFFSET: u32 = CK_CHANNEL1_CONST_OFFSET + VEC4_SIZE;
const PUSH_CONSTANTS_SIZE: u32 = CK_CHANNEL2_CONST_OFFSET + VEC4_SIZE;

const _: () = assert!(
    PUSH_CONSTANTS_SIZE <= MAX_PUSH_CONSTANTS_SIZE,
    "Push constants size exceeds the maximum size"
);

pub struct StreamViewParams {
    pub swapchain_index: u32,
    pub input_view_params: ViewParams,
    pub output_view_params: ViewParams,
}

/// Per-view center_shift uniform for the FFE_RUNTIME path. 16 bytes (vec2 +
/// pad) to satisfy uniform buffer size/alignment; the shader reads only the
/// first two floats.
const FFE_RUNTIME_UNIFORM_SIZE: u64 = 16;

#[derive(Debug)]
struct ViewObjects {
    bind_group: BindGroup,
    render_target: Vec<TextureView>,
}

/// The dormant per-view foveation pipeline (see `stream.wgsl` FFE_RUNTIME). Only
/// built when foveated encoding is enabled, only used when `render` is given a
/// per-view `center_shift`. Mirrors the static `pipeline`/`views_objects` but
/// derives the de-foveation constants at runtime from a per-view uniform, so
/// each eye can carry its own (eye-tracked) inset centre.
#[derive(Debug)]
struct FfeRuntime {
    pipeline: RenderPipeline,
    views: [FfeRuntimeView; 2],
}

#[derive(Debug)]
struct FfeRuntimeView {
    bind_group: BindGroup,
    center_shift_uniform: Buffer,
}

pub struct StreamRenderer {
    context: Rc<GraphicsContext>,
    staging_renderer: StagingRenderer,
    pipeline: RenderPipeline,
    views_objects: [ViewObjects; 2],
    ffe_runtime: Option<FfeRuntime>,
}

impl StreamRenderer {
    #[expect(clippy::too_many_arguments)]
    #[cfg_attr(any(target_os = "macos", target_os = "ios"), expect(unused))]
    pub fn new(
        context: Rc<GraphicsContext>,
        base_view_resolution: UVec2,
        target_view_resolution: UVec2,
        swapchain_textures: [Vec<u32>; 2],
        target_format: u32,
        foveated_encoding: Option<FoveatedEncodingConfig>,
        enable_srgb_correction: bool,
        fix_limited_range: bool,
        encoding_gamma: f32,
        upscaling: Option<UpscalingConfig>,
    ) -> Self {
        let device = &context.device;

        let target_format = super::gl_format_to_wgpu(target_format);

        let bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        sample_type: TextureSampleType::Float { filterable: true },
                        view_dimension: TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let shader_module = device.create_shader_module(include_wgsl!("../resources/stream.wgsl"));

        let mut constants = vec![];

        constants.extend([
            ("ENABLE_SRGB_CORRECTION", enable_srgb_correction.into()),
            ("ENCODING_GAMMA", encoding_gamma.into()),
        ]);

        let staging_resolution = if let Some(foveated_encoding) = &foveated_encoding {
            let (staging_resolution, ffe_constants) =
                foveated_encoding_shader_constants(base_view_resolution, foveated_encoding.clone());
            constants.extend(ffe_constants);

            staging_resolution
        } else {
            base_view_resolution
        };

        if let Some(upscaling) = &upscaling {
            constants.extend([
                ("ENABLE_UPSCALING", true.into()),
                (
                    "UPSCALE_USE_EDGE_DIRECTION",
                    upscaling.edge_direction.into(),
                ),
                (
                    "UPSCALE_EDGE_THRESHOLD",
                    (upscaling.edge_threshold / 255.0).into(),
                ),
                ("UPSCALE_EDGE_SHARPNESS", upscaling.edge_sharpness.into()),
            ]);
        };

        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: None,
            // Note: Layout cannot be inferred because of a bug with push constants
            layout: Some(&device.create_pipeline_layout(&PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[&bind_group_layout],
                push_constant_ranges: &[PushConstantRange {
                    stages: ShaderStages::VERTEX_FRAGMENT,
                    range: 0..PUSH_CONSTANTS_SIZE,
                }],
            })),
            vertex: VertexState {
                module: &shader_module,
                entry_point: None,
                compilation_options: PipelineCompilationOptions {
                    constants: &constants,
                    zero_initialize_workgroup_memory: false,
                },
                buffers: &[],
            },
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(FragmentState {
                module: &shader_module,
                entry_point: None,
                compilation_options: PipelineCompilationOptions {
                    constants: &constants,
                    zero_initialize_workgroup_memory: false,
                },
                targets: &[Some(ColorTargetState {
                    format: target_format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        let sampler = device.create_sampler(&SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Dormant per-view foveation pipeline + its bind group layout. Built only when foveated
        // encoding is enabled; the static `pipeline`/`bind_group_layout` above are untouched.
        let ffe_runtime_parts: Option<(RenderPipeline, BindGroupLayout)> =
            foveated_encoding.as_ref().map(|fe| {
                build_ffe_runtime_pipeline(
                    device,
                    &shader_module,
                    target_format,
                    base_view_resolution,
                    fe,
                    enable_srgb_correction,
                    encoding_gamma,
                    upscaling.as_ref(),
                )
            });

        let mut view_objects = vec![];
        let mut ffe_runtime_views: Vec<FfeRuntimeView> = vec![];
        let mut staging_textures_gl = vec![];
        for target_swapchain in &swapchain_textures {
            let staging_texture = super::create_texture(device, staging_resolution, target_format);
            let staging_view = staging_texture.create_view(&TextureViewDescriptor::default());

            let bind_group = device.create_bind_group(&BindGroupDescriptor {
                label: None,
                layout: &bind_group_layout,
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::TextureView(&staging_view),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::Sampler(&sampler),
                    },
                ],
            });

            if let Some((_, ffe_bind_group_layout)) = &ffe_runtime_parts {
                let center_shift_uniform = device.create_buffer(&BufferDescriptor {
                    label: Some("ffe_runtime_center_shift"),
                    size: FFE_RUNTIME_UNIFORM_SIZE,
                    usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });

                let ffe_bind_group = device.create_bind_group(&BindGroupDescriptor {
                    label: Some("ffe_runtime_bind_group"),
                    layout: ffe_bind_group_layout,
                    entries: &[
                        BindGroupEntry {
                            binding: 0,
                            resource: BindingResource::TextureView(&staging_view),
                        },
                        BindGroupEntry {
                            binding: 1,
                            resource: BindingResource::Sampler(&sampler),
                        },
                        BindGroupEntry {
                            binding: 2,
                            resource: center_shift_uniform.as_entire_binding(),
                        },
                    ],
                });

                ffe_runtime_views.push(FfeRuntimeView {
                    bind_group: ffe_bind_group,
                    center_shift_uniform,
                });
            }

            let render_target = super::create_gl_swapchain(
                device,
                target_swapchain,
                target_view_resolution,
                target_format,
            );

            view_objects.push(ViewObjects {
                bind_group,
                render_target,
            });

            #[cfg(not(any(target_os = "macos", target_os = "ios")))]
            {
                let staging_texture_gl = unsafe {
                    staging_texture.as_hal::<wgpu::hal::api::Gles, _, _>(|tex| {
                        let wgpu::hal::gles::TextureInner::Texture { raw, .. } = tex.unwrap().inner
                        else {
                            panic!("invalid texture type");
                        };
                        raw
                    })
                };
                staging_textures_gl.push(staging_texture_gl);
            }
        }

        let staging_renderer = StagingRenderer::new(
            Rc::clone(&context),
            staging_textures_gl.try_into().unwrap(),
            staging_resolution,
            fix_limited_range,
        );

        let ffe_runtime = ffe_runtime_parts.map(|(pipeline, _layout)| FfeRuntime {
            pipeline,
            views: ffe_runtime_views.try_into().unwrap(),
        });

        Self {
            context,
            staging_renderer,
            pipeline,
            views_objects: view_objects.try_into().unwrap(),
            ffe_runtime,
        }
    }

    /// # Safety
    /// `hardware_buffer` must be a valid pointer to a ANativeWindowBuffer.
    /// `per_view_center_shift` carries the eye-tracked per-view foveation inset
    /// centre for each eye (left = index 0). When `Some` and the runtime FFE
    /// pipeline exists (foveated encoding enabled), the de-foveation warp is
    /// recomputed per eye from these. `None` (today's only state — no producer
    /// ships per-view foveation yet) uses the static baked-constant pipeline
    /// unchanged.
    pub fn render(
        &self,
        hardware_buffer: *mut c_void,
        view_params: [StreamViewParams; 2],
        passthrough: Option<&PassthroughMode>,
        per_view_center_shift: Option<[Vec2; 2]>,
    ) {
        // if hardware_buffer is available copy stream to staging texture
        if !hardware_buffer.is_null() {
            self.staging_renderer.render(hardware_buffer);
        }

        let mut encoder = self
            .context
            .device
            .create_command_encoder(&Default::default());

        for (view_idx, view_params) in view_params.iter().enumerate() {
            let mut render_pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &self.views_objects[view_idx].render_target
                        [view_params.swapchain_index as usize],
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: LoadOp::Clear(Color::BLACK),
                        store: StoreOp::Store,
                    },
                })],
                ..Default::default()
            });

            let input_fov = view_params.input_view_params.fov;

            let tanl = f32::tan(input_fov.left);
            let tanr = f32::tan(input_fov.right);
            let tanu = f32::tan(input_fov.up);
            let tand = f32::tan(input_fov.down);

            let width = tanr - tanl;
            let height = tanu - tand;
            let quad_depth = 1000.0;

            let output_mat4 = Mat4::from_translation(view_params.output_view_params.pose.position)
                * Mat4::from_quat(view_params.output_view_params.pose.orientation);
            let input_mat4 = Mat4::from_translation(view_params.input_view_params.pose.position)
                * Mat4::from_quat(view_params.input_view_params.pose.orientation);

            // The image is at z = -1.0, so we use tangents for the size
            let model_mat =
                Mat4::from_translation(Vec3 {
                    x: 0.0,
                    y: 0.0,
                    z: -quad_depth * 0.5,
                }) * Mat4::from_scale(Vec3::new(quad_depth, quad_depth, quad_depth * 0.5))
                    * Mat4::from_translation(Vec3::new(
                        width / 2.0 + tanl,
                        height / 2.0 + tand,
                        -1.0,
                    ))
                    * Mat4::from_scale(Vec3::new(width, height, 1.));
            let view_mat = output_mat4.inverse() * input_mat4;
            let proj_mat = super::projection_from_fov(view_params.output_view_params.fov);

            let transform = proj_mat * view_mat * model_mat;

            let transform_bytes = transform
                .to_cols_array()
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<u8>>();

            // Per-view foveation: only when the caller supplied per-view shifts AND the runtime
            // pipeline was built. Otherwise the static (baked-constant) pipeline runs unchanged.
            let use_runtime_ffe = match (&self.ffe_runtime, per_view_center_shift) {
                (Some(ffe), Some(shifts)) => {
                    let shift = shifts[view_idx];
                    let shift_bytes = [shift.x, shift.y, 0.0f32, 0.0f32]
                        .iter()
                        .flat_map(|v| v.to_le_bytes())
                        .collect::<Vec<u8>>();
                    self.context.queue.write_buffer(
                        &ffe.views[view_idx].center_shift_uniform,
                        0,
                        &shift_bytes,
                    );
                    render_pass.set_pipeline(&ffe.pipeline);
                    render_pass.set_bind_group(0, &ffe.views[view_idx].bind_group, &[]);
                    true
                }
                _ => false,
            };
            if !use_runtime_ffe {
                render_pass.set_pipeline(&self.pipeline);
                render_pass.set_bind_group(0, &self.views_objects[view_idx].bind_group, &[]);
            }

            render_pass.set_push_constants(
                ShaderStages::VERTEX_FRAGMENT,
                TRANSFORM_CONST_OFFSET,
                &transform_bytes,
            );
            render_pass.set_push_constants(
                ShaderStages::VERTEX_FRAGMENT,
                VIEW_INDEX_CONST_OFFSET,
                &(view_idx as u32).to_le_bytes(),
            );
            set_passthrough_push_constants(&mut render_pass, passthrough);
            render_pass.draw(0..4, 0..1);
        }

        self.context.queue.submit(iter::once(encoder.finish()));
    }
}

/// Builds the dormant per-view foveation pipeline (see `stream.wgsl` FFE_RUNTIME) and its bind
/// group layout. Spec-constants carry the resolution-coupled (hence static) `CENTER_SIZE_*`,
/// `VIEW_*_RATIO`, `EDGE_*_RATIO` from the aligned foveation math; the per-view `center_shift`
/// arrives at render time via the binding-2 uniform, from which the shader derives the de-foveation
/// constants. Kept separate from the static pipeline so that path is untouched.
#[expect(clippy::too_many_arguments)]
fn build_ffe_runtime_pipeline(
    device: &wgpu::Device,
    shader_module: &ShaderModule,
    target_format: wgpu::TextureFormat,
    base_view_resolution: UVec2,
    config: &FoveatedEncodingConfig,
    enable_srgb_correction: bool,
    encoding_gamma: f32,
    upscaling: Option<&UpscalingConfig>,
) -> (RenderPipeline, BindGroupLayout) {
    let compress = foveation_compress_vars(base_view_resolution, config.clone());

    let mut constants = vec![
        ("ENABLE_SRGB_CORRECTION", f64::from(enable_srgb_correction)),
        ("ENCODING_GAMMA", f64::from(encoding_gamma)),
        ("FFE_RUNTIME", 1.0),
        ("CENTER_SIZE_X", f64::from(compress.center_size[0])),
        ("CENTER_SIZE_Y", f64::from(compress.center_size[1])),
        ("VIEW_WIDTH_RATIO", f64::from(compress.eye_size_ratio[0])),
        ("VIEW_HEIGHT_RATIO", f64::from(compress.eye_size_ratio[1])),
        ("EDGE_X_RATIO", f64::from(compress.edge_ratio[0])),
        ("EDGE_Y_RATIO", f64::from(compress.edge_ratio[1])),
    ];
    if let Some(upscaling) = upscaling {
        constants.extend([
            ("ENABLE_UPSCALING", 1.0),
            (
                "UPSCALE_USE_EDGE_DIRECTION",
                f64::from(upscaling.edge_direction),
            ),
            (
                "UPSCALE_EDGE_THRESHOLD",
                f64::from(upscaling.edge_threshold / 255.0),
            ),
            (
                "UPSCALE_EDGE_SHARPNESS",
                f64::from(upscaling.edge_sharpness),
            ),
        ]);
    }

    let bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("ffe_runtime_bind_group_layout"),
        entries: &[
            BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Texture {
                    sample_type: TextureSampleType::Float { filterable: true },
                    view_dimension: TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 1,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Sampler(SamplerBindingType::Filtering),
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 2,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });

    let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("ffe_runtime_pipeline"),
        layout: Some(&device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[PushConstantRange {
                stages: ShaderStages::VERTEX_FRAGMENT,
                range: 0..PUSH_CONSTANTS_SIZE,
            }],
        })),
        vertex: VertexState {
            module: shader_module,
            entry_point: None,
            compilation_options: PipelineCompilationOptions {
                constants: &constants,
                zero_initialize_workgroup_memory: false,
            },
            buffers: &[],
        },
        primitive: PrimitiveState {
            topology: PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(FragmentState {
            module: shader_module,
            entry_point: None,
            compilation_options: PipelineCompilationOptions {
                constants: &constants,
                zero_initialize_workgroup_memory: false,
            },
            targets: &[Some(ColorTargetState {
                format: target_format,
                blend: None,
                write_mask: ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    });

    (pipeline, bind_group_layout)
}

fn set_passthrough_push_constants(render_pass: &mut RenderPass, config: Option<&PassthroughMode>) {
    const DEG_TO_NORM: f32 = 1. / 360.;

    fn set_u32(render_pass: &mut RenderPass, offset: u32, value: u32) {
        render_pass.set_push_constants(ShaderStages::VERTEX_FRAGMENT, offset, &value.to_le_bytes());
    }

    fn set_float(render_pass: &mut RenderPass, offset: u32, value: f32) {
        render_pass.set_push_constants(ShaderStages::VERTEX_FRAGMENT, offset, &value.to_le_bytes());
    }

    fn set_vec4(render_pass: &mut RenderPass, offset: u32, value: Vec4) {
        render_pass.set_push_constants(
            ShaderStages::VERTEX_FRAGMENT,
            offset,
            &value.x.to_le_bytes(),
        );
        render_pass.set_push_constants(
            ShaderStages::VERTEX_FRAGMENT,
            offset + FLOAT_SIZE,
            &value.y.to_le_bytes(),
        );
        render_pass.set_push_constants(
            ShaderStages::VERTEX_FRAGMENT,
            offset + 2 * FLOAT_SIZE,
            &value.z.to_le_bytes(),
        );
        render_pass.set_push_constants(
            ShaderStages::VERTEX_FRAGMENT,
            offset + 3 * FLOAT_SIZE,
            &value.w.to_le_bytes(),
        );
    }

    match config {
        None => {
            set_u32(render_pass, PASSTHROUGH_MODE_OFFSET, 0);
            set_float(render_pass, ALPHA_CONST_OFFSET, 1.);
        }
        Some(PassthroughMode::Blend { threshold, .. }) => {
            set_u32(render_pass, PASSTHROUGH_MODE_OFFSET, 0);
            set_float(render_pass, ALPHA_CONST_OFFSET, 1. - threshold);
        }
        Some(PassthroughMode::RgbChromaKey(config)) => {
            set_u32(render_pass, PASSTHROUGH_MODE_OFFSET, 1);

            let norm = |v| v as f32 / 255.;

            let red = norm(config.red);
            let green = norm(config.green);
            let blue = norm(config.blue);

            let thresh = norm(config.distance_threshold);

            let up_feather = 1. + config.feathering;
            let down_feather = 1. - config.feathering;

            let range_vec =
                thresh * Vec4::new(-up_feather, -down_feather, down_feather, up_feather);

            set_vec4(render_pass, CK_CHANNEL0_CONST_OFFSET, red + range_vec);
            set_vec4(render_pass, CK_CHANNEL1_CONST_OFFSET, green + range_vec);
            set_vec4(render_pass, CK_CHANNEL2_CONST_OFFSET, blue + range_vec);
        }
        Some(PassthroughMode::HsvChromaKey(config)) => {
            set_u32(render_pass, PASSTHROUGH_MODE_OFFSET, 2);

            set_vec4(
                render_pass,
                CK_CHANNEL0_CONST_OFFSET,
                Vec4::new(
                    config.hue_start_max_deg,
                    config.hue_start_min_deg,
                    config.hue_end_min_deg,
                    config.hue_end_max_deg,
                ) * DEG_TO_NORM,
            );

            set_vec4(
                render_pass,
                CK_CHANNEL1_CONST_OFFSET,
                Vec4::new(
                    config.saturation_start_max,
                    config.saturation_start_min,
                    config.saturation_end_min,
                    config.saturation_end_max,
                ),
            );

            set_vec4(
                render_pass,
                CK_CHANNEL2_CONST_OFFSET,
                Vec4::new(
                    config.value_start_max,
                    config.value_start_min,
                    config.value_end_min,
                    config.value_end_max,
                ),
            );
        }
    }
}

// The canonical foveated-encoding math now lives in `alvr_session` so the
// wgpu-free OpenXR-mode encoder bridge can call the exact same function the
// client renderer uses. Re-exported here to keep `alvr_graphics::
// foveated_encoding_shader_constants` working unchanged.
pub use alvr_session::foveated_encoding_shader_constants;

pub fn compute_target_view_resolution(
    resolution: UVec2,
    upscaling: &Option<UpscalingConfig>,
) -> UVec2 {
    let mut target_resolution = resolution.as_vec2();
    if let Some(upscaling) = upscaling {
        target_resolution *= upscaling.upscale_factor;
    }
    target_resolution.as_uvec2()
}

#[cfg(test)]
mod tests {
    /// `stream.wgsl` is only compiled at pipeline-creation time on a real device, so a typo in the
    /// new FFE_RUNTIME path wouldn't surface until a headset runs it. naga (re-exported by wgpu)
    /// front-parses and validates the WGSL here — the one guardrail available without a GPU. This
    /// validates the whole shader (both the static ENABLE_FFE and the new FFE_RUNTIME branches, plus
    /// the binding-2 uniform) with override defaults.
    #[test]
    fn stream_shader_parses_and_validates() {
        use wgpu::naga;

        let src = include_str!("../resources/stream.wgsl");
        let module = naga::front::wgsl::parse_str(src).expect("stream.wgsl must parse");

        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("stream.wgsl must validate");
    }
}
