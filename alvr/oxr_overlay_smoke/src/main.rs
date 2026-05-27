// Smoke test for Phase 7 Slice 2 — exercises comp_alvr::compose_via_squasher
// by submitting an XR_TYPE_COMPOSITION_LAYER_PROJECTION + XR_TYPE_COMPOSITION_
// LAYER_QUAD pair through xrEndFrame against a running monado-service.exe.
//
// This is the smallest viable OpenXR Vulkan-binding client that reaches the
// squasher dispatch path. It allocates swapchains but never renders into
// them — acquire/wait/release each frame to satisfy the swapchain protocol,
// then submit. The compositor server side sees real projection + quad
// layers and runs comp_alvr::layer_commit (and compose_via_squasher when
// 2c.1 wires the distortion target) exactly as a production game would.
//
// Verification (after 30 successful frames):
//   - smoke output: `Summary: submitted=30 endframe_errors=0 final_state=FOCUSED`
//   - monado-service.log: no `VK_ERROR_*` / no assertion fires from the
//     squasher path; the session-state machine walks IDLE -> READY ->
//     SYNCHRONIZED -> VISIBLE -> FOCUSED.
//
// State-machine note: the smoke client deliberately does NOT gate frame
// submission on session_state. xrWaitFrame / xrBeginFrame / xrEndFrame only
// verify SESSION_RUNNING (set by xrBeginSession); the READY -> SYNCHRONIZED
// transition is driven from inside xrEndFrame itself
// (do_synchronize_state_change in oxr_session_frame_end.c). Gating
// xrEndFrame on SYNCHRONIZED would deadlock.
//
// Run:
//   set XR_RUNTIME_JSON=<path to active_runtime_alvr.json>   (or HKLM)
//   cargo run -p alvr_oxr_overlay_smoke --release
//
// Windows quirk: PS-remoting puts the process in an elevated context,
// where the OpenXR loader IGNORES XR_RUNTIME_JSON. From a remoting
// session, the only way to point at Monado is HKLM\Software\Khronos\
// OpenXR\1\ActiveRuntime; restore the previous value when you're done.

use ash::vk;
use ash::vk::Handle;
use openxr as xr;
use std::{
    error::Error,
    ffi::CString,
    time::{Duration, Instant},
};

const APP_NAME: &str = "oxr_overlay_smoke";
const VIEW_COUNT: usize = 2;
const QUAD_SIZE_PX: u32 = 512;
const TARGET_FRAMES: u32 = 30;
const DEADLINE: Duration = Duration::from_secs(12);

fn main() -> Result<(), Box<dyn Error>> {
    println!("=== oxr_overlay_smoke ===");
    println!(
        "XR_RUNTIME_JSON = {:?}",
        std::env::var("XR_RUNTIME_JSON").ok()
    );

    let xr_entry = unsafe { xr::Entry::load()? };
    let available = xr_entry.enumerate_extensions()?;
    if !available.khr_vulkan_enable {
        return Err("XR_KHR_vulkan_enable not available".into());
    }
    println!("XR_KHR_vulkan_enable available = true");

    let mut enabled = xr::ExtensionSet::default();
    enabled.khr_vulkan_enable = true;
    let xr_instance = xr_entry.create_instance(
        &xr::ApplicationInfo {
            application_name: APP_NAME,
            application_version: 1,
            engine_name: "alvr",
            engine_version: 0,
            api_version: xr::Version::new(1, 0, 34),
        },
        &enabled,
        &[],
    )?;
    let ip = xr_instance.properties()?;
    println!("Runtime: {} v{}", ip.runtime_name, ip.runtime_version);

    let system = xr_instance.system(xr::FormFactor::HEAD_MOUNTED_DISPLAY)?;
    let sp = xr_instance.system_properties(system)?;
    println!(
        "System : id={} name={:?} vendor=0x{:x}",
        sp.system_id.into_raw(),
        sp.system_name,
        sp.vendor_id
    );

    // Vulkan setup driven by the OpenXR runtime's stated requirements. We
    // honour min_api_version_supported by clamping our requested Vulkan API
    // version to it (Monado today asks for 1.0).
    let reqs = xr_instance.graphics_requirements::<xr::Vulkan>(system)?;
    println!(
        "Vulkan reqs: min={} max={}",
        reqs.min_api_version_supported, reqs.max_api_version_supported
    );

    let want_vk_version = vk::API_VERSION_1_1;
    if (want_vk_version as u64)
        < ((reqs.min_api_version_supported.major() as u64) << 22
            | (reqs.min_api_version_supported.minor() as u64) << 12)
    {
        return Err(format!("VK 1.1 below OpenXR min {}", reqs.min_api_version_supported).into());
    }

    let vk_instance_ext_str = xr_instance.vulkan_legacy_instance_extensions(system)?;
    let vk_instance_exts: Vec<&str> = vk_instance_ext_str.split_whitespace().collect();
    println!(
        "VK instance extensions required by OpenXR: {:?}",
        vk_instance_exts
    );

    let vk_entry = unsafe { ash::Entry::load()? };
    let app_name_c = CString::new(APP_NAME)?;
    let engine_name_c = CString::new("alvr")?;
    let vk_app_info = vk::ApplicationInfo::default()
        .application_name(&app_name_c)
        .application_version(1)
        .engine_name(&engine_name_c)
        .engine_version(0)
        .api_version(want_vk_version);

    let inst_ext_cstrings: Vec<CString> = vk_instance_exts
        .iter()
        .map(|s| CString::new(*s).unwrap())
        .collect();
    let inst_ext_ptrs: Vec<*const i8> = inst_ext_cstrings.iter().map(|c| c.as_ptr()).collect();

    let vk_inst_create_info = vk::InstanceCreateInfo::default()
        .application_info(&vk_app_info)
        .enabled_extension_names(&inst_ext_ptrs);
    let vk_instance = unsafe { vk_entry.create_instance(&vk_inst_create_info, None)? };

    // Physical device — OpenXR tells us which to use.
    let raw_phys =
        unsafe { xr_instance.vulkan_graphics_device(system, vk_instance.handle().as_raw() as _)? };
    let physical_device = vk::PhysicalDevice::from_raw(raw_phys as u64);
    let pd_props = unsafe { vk_instance.get_physical_device_properties(physical_device) };
    let name_str = unsafe {
        std::ffi::CStr::from_ptr(pd_props.device_name.as_ptr())
            .to_string_lossy()
            .into_owned()
    };
    println!(
        "VK physical device: {} (vendor 0x{:x})",
        name_str, pd_props.vendor_id
    );

    // Graphics queue family.
    let qfs = unsafe { vk_instance.get_physical_device_queue_family_properties(physical_device) };
    let graphics_family = qfs
        .iter()
        .enumerate()
        .find(|(_, q)| q.queue_flags.contains(vk::QueueFlags::GRAPHICS))
        .map(|(i, _)| i as u32)
        .ok_or("no graphics queue family")?;

    let vk_device_ext_str = xr_instance.vulkan_legacy_device_extensions(system)?;
    let vk_device_exts: Vec<&str> = vk_device_ext_str.split_whitespace().collect();
    println!(
        "VK device extensions required by OpenXR: {:?}",
        vk_device_exts
    );

    let dev_ext_cstrings: Vec<CString> = vk_device_exts
        .iter()
        .map(|s| CString::new(*s).unwrap())
        .collect();
    let dev_ext_ptrs: Vec<*const i8> = dev_ext_cstrings.iter().map(|c| c.as_ptr()).collect();

    let queue_priorities = [1.0f32];
    let queue_create_info = vk::DeviceQueueCreateInfo::default()
        .queue_family_index(graphics_family)
        .queue_priorities(&queue_priorities);
    let queue_create_infos = [queue_create_info];
    let dev_create_info = vk::DeviceCreateInfo::default()
        .queue_create_infos(&queue_create_infos)
        .enabled_extension_names(&dev_ext_ptrs);
    let vk_device = unsafe { vk_instance.create_device(physical_device, &dev_create_info, None)? };

    // Queue + a one-shot command buffer used to clear each acquired swapchain
    // image to a (cycling) colour before release, so the streamed frames carry
    // visible content instead of uninitialised memory. This makes the smoke a
    // real end-to-end pixel test of the encode path, not just a protocol smoke.
    let vk_queue = unsafe { vk_device.get_device_queue(graphics_family, 0) };
    let cmd_pool = unsafe {
        vk_device.create_command_pool(
            &vk::CommandPoolCreateInfo::default()
                .queue_family_index(graphics_family)
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
            None,
        )?
    };
    let cmd_buf = unsafe {
        vk_device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(cmd_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1),
        )?[0]
    };

    // Clear `image` to `color` via a transfer clear (UNDEFINED -> TRANSFER_DST ->
    // COLOR_ATTACHMENT), submitted synchronously. Cheap and CPU-stalling, which
    // is fine for a smoke.
    let clear_image =
        |image: vk::Image, color: [f32; 4]| -> Result<(), Box<dyn std::error::Error>> {
            let range = vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1);
            unsafe {
                vk_device.reset_command_buffer(cmd_buf, vk::CommandBufferResetFlags::empty())?;
                vk_device.begin_command_buffer(
                    cmd_buf,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )?;
                let to_dst = vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(image)
                    .subresource_range(range);
                vk_device.cmd_pipeline_barrier(
                    cmd_buf,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[to_dst],
                );
                vk_device.cmd_clear_color_image(
                    cmd_buf,
                    image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &vk::ClearColorValue { float32: color },
                    &[range],
                );
                let to_color = vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(image)
                    .subresource_range(range);
                vk_device.cmd_pipeline_barrier(
                    cmd_buf,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[to_color],
                );
                vk_device.end_command_buffer(cmd_buf)?;
                let cbs = [cmd_buf];
                let submit = vk::SubmitInfo::default().command_buffers(&cbs);
                vk_device.queue_submit(vk_queue, &[submit], vk::Fence::null())?;
                vk_device.queue_wait_idle(vk_queue)?;
            }
            Ok(())
        };

    // OpenXR session over the Vulkan binding.
    let (session, mut frame_waiter, mut frame_stream) = unsafe {
        xr_instance.create_session::<xr::Vulkan>(
            system,
            &xr::vulkan::SessionCreateInfo {
                instance: vk_instance.handle().as_raw() as _,
                physical_device: physical_device.as_raw() as _,
                device: vk_device.handle().as_raw() as _,
                queue_family_index: graphics_family,
                queue_index: 0,
            },
        )?
    };
    println!("Vulkan-binding session created");

    session.begin(xr::ViewConfigurationType::PRIMARY_STEREO)?;
    println!("session.begin() ok");

    // Reference space (LOCAL is the safest minimum — STAGE may be unavailable
    // until the space overseer has been told a floor offset, but LOCAL is
    // always available).
    let spaces = session.enumerate_reference_spaces()?;
    println!("Supported reference spaces: {:?}", spaces);
    let space_type = if spaces.contains(&xr::ReferenceSpaceType::STAGE) {
        xr::ReferenceSpaceType::STAGE
    } else if spaces.contains(&xr::ReferenceSpaceType::LOCAL) {
        xr::ReferenceSpaceType::LOCAL
    } else {
        xr::ReferenceSpaceType::VIEW
    };
    let space = session.create_reference_space(space_type, xr::Posef::IDENTITY)?;

    // View configuration recommended dims.
    let view_configs = xr_instance
        .enumerate_view_configuration_views(system, xr::ViewConfigurationType::PRIMARY_STEREO)?;
    let proj_w = view_configs[0].recommended_image_rect_width;
    let proj_h = view_configs[0].recommended_image_rect_height;
    println!("Projection view dims: {}x{}", proj_w, proj_h);

    // Pick an sRGB swapchain format.
    let formats = session.enumerate_swapchain_formats()?;
    println!("Swapchain formats: {:?}", formats);
    let want = vk::Format::R8G8B8A8_SRGB.as_raw() as u32;
    let alt = vk::Format::B8G8R8A8_SRGB.as_raw() as u32;
    let swapchain_format = if formats.contains(&want) {
        want
    } else if formats.contains(&alt) {
        alt
    } else {
        *formats.first().ok_or("no swapchain formats")?
    };
    println!("Chose swapchain format = {}", swapchain_format);

    let mk_proj_swapchain = || -> Result<xr::Swapchain<xr::Vulkan>, xr::sys::Result> {
        session.create_swapchain(&xr::SwapchainCreateInfo {
            create_flags: xr::SwapchainCreateFlags::EMPTY,
            usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT
                | xr::SwapchainUsageFlags::SAMPLED
                | xr::SwapchainUsageFlags::TRANSFER_DST,
            format: swapchain_format,
            sample_count: 1,
            width: proj_w,
            height: proj_h,
            face_count: 1,
            array_size: 1,
            mip_count: 1,
        })
    };
    let mut proj_swapchains: Vec<xr::Swapchain<xr::Vulkan>> = (0..VIEW_COUNT)
        .map(|_| mk_proj_swapchain())
        .collect::<Result<_, _>>()?;

    let mut quad_swapchain: xr::Swapchain<xr::Vulkan> =
        session.create_swapchain(&xr::SwapchainCreateInfo {
            create_flags: xr::SwapchainCreateFlags::EMPTY,
            usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT
                | xr::SwapchainUsageFlags::SAMPLED,
            format: swapchain_format,
            sample_count: 1,
            width: QUAD_SIZE_PX,
            height: QUAD_SIZE_PX,
            face_count: 1,
            array_size: 1,
            mip_count: 1,
        })?;
    println!("Swapchains created: 2 projection + 1 quad");

    // Underlying VkImages per swapchain, indexed by the acquired image index.
    let proj_images: Vec<Vec<vk::Image>> = proj_swapchains
        .iter()
        .map(|sc| -> Result<Vec<vk::Image>, Box<dyn std::error::Error>> {
            Ok(sc
                .enumerate_images()?
                .into_iter()
                .map(vk::Image::from_raw)
                .collect())
        })
        .collect::<Result<_, _>>()?;
    let quad_images: Vec<vk::Image> = quad_swapchain
        .enumerate_images()?
        .into_iter()
        .map(vk::Image::from_raw)
        .collect();

    // Frame loop — drain events until SYNCHRONIZED/VISIBLE/FOCUSED, then
    // submit overlay frames until either TARGET_FRAMES or DEADLINE.
    let mut storage = xr::EventDataBuffer::new();
    let mut session_state = xr::SessionState::UNKNOWN;
    let start = Instant::now();
    let mut submitted = 0u32;
    let mut endframe_errors = 0u32;

    // Defaults give the original 30-frame protocol smoke. Override via env for a
    // long continuous stream to eyeball the decoded image on the headset, e.g.
    // OXR_SMOKE_FRAMES=100000 OXR_SMOKE_SECS=600.
    let target_frames = std::env::var("OXR_SMOKE_FRAMES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(TARGET_FRAMES);
    let deadline = std::env::var("OXR_SMOKE_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEADLINE);

    while submitted < target_frames && start.elapsed() < deadline {
        while let Some(ev) = xr_instance.poll_event(&mut storage)? {
            if let xr::Event::SessionStateChanged(e) = ev
                && e.state() != session_state
            {
                session_state = e.state();
                println!("  session state -> {:?}", session_state);
            }
        }
        // Do NOT gate frame submission on the OpenXR session state — the
        // verify checks in oxr_api_session.c only require SESSION_RUNNING
        // (set by xrBeginSession). The READY -> SYNCHRONIZED transition is
        // driven by xrEndFrame itself (do_synchronize_state_change in
        // oxr_session_frame_end.c), so we'd be stuck in a deadlock waiting
        // for a state we can only reach by submitting.

        let frame_state = frame_waiter.wait()?;
        frame_stream.begin()?;

        if !frame_state.should_render {
            frame_stream.end(
                frame_state.predicted_display_time,
                xr::EnvironmentBlendMode::OPAQUE,
                &[],
            )?;
            continue;
        }

        // Cycle each swapchain image. We don't render — the OpenXR spec is
        // happy with "acquire / wait / release" as long as the layer references
        // a valid index. The compositor reads from the released image; what's
        // in it (uninitialised cleared memory) doesn't affect whether the
        // squasher dispatches.
        // Cycle a colour each frame so the streamed image is unmistakably live
        // on the headset (RGB phase-shifted by 120°). The quad gets the inverse
        // colour so both composited layers are distinguishable.
        let t = submitted as f32 * 0.06;
        let proj_color = [
            0.5 + 0.5 * t.sin(),
            0.5 + 0.5 * (t + 2.094).sin(),
            0.5 + 0.5 * (t + 4.188).sin(),
            1.0,
        ];
        let quad_color = [
            1.0 - proj_color[0],
            1.0 - proj_color[1],
            1.0 - proj_color[2],
            1.0,
        ];

        for (v, sc) in proj_swapchains.iter_mut().enumerate() {
            let idx = sc.acquire_image()?;
            sc.wait_image(xr::Duration::INFINITE)?;
            clear_image(proj_images[v][idx as usize], proj_color)?;
            sc.release_image()?;
        }
        let qidx = quad_swapchain.acquire_image()?;
        quad_swapchain.wait_image(xr::Duration::INFINITE)?;
        clear_image(quad_images[qidx as usize], quad_color)?;
        quad_swapchain.release_image()?;

        let (view_flags, located_views) = session.locate_views(
            xr::ViewConfigurationType::PRIMARY_STEREO,
            frame_state.predicted_display_time,
            &space,
        )?;
        let pose_ok = view_flags.contains(xr::ViewStateFlags::POSITION_VALID)
            && view_flags.contains(xr::ViewStateFlags::ORIENTATION_VALID);
        if !pose_ok || located_views.len() < VIEW_COUNT {
            frame_stream.end(
                frame_state.predicted_display_time,
                xr::EnvironmentBlendMode::OPAQUE,
                &[],
            )?;
            continue;
        }

        let proj_views: Vec<xr::CompositionLayerProjectionView<xr::Vulkan>> = (0..VIEW_COUNT)
            .map(|v| {
                xr::CompositionLayerProjectionView::new()
                    .pose(located_views[v].pose)
                    .fov(located_views[v].fov)
                    .sub_image(
                        xr::SwapchainSubImage::new()
                            .swapchain(&proj_swapchains[v])
                            .image_array_index(0)
                            .image_rect(xr::Rect2Di {
                                offset: xr::Offset2Di { x: 0, y: 0 },
                                extent: xr::Extent2Di {
                                    width: proj_w as i32,
                                    height: proj_h as i32,
                                },
                            }),
                    )
            })
            .collect();

        let proj_layer = xr::CompositionLayerProjection::new()
            .space(&space)
            .views(&proj_views);
        let quad_layer = xr::CompositionLayerQuad::new()
            .space(&space)
            .eye_visibility(xr::EyeVisibility::BOTH)
            .sub_image(
                xr::SwapchainSubImage::new()
                    .swapchain(&quad_swapchain)
                    .image_array_index(0)
                    .image_rect(xr::Rect2Di {
                        offset: xr::Offset2Di { x: 0, y: 0 },
                        extent: xr::Extent2Di {
                            width: QUAD_SIZE_PX as i32,
                            height: QUAD_SIZE_PX as i32,
                        },
                    }),
            )
            .pose(xr::Posef {
                position: xr::Vector3f {
                    x: 0.0,
                    y: 0.0,
                    z: -1.5,
                },
                orientation: xr::Quaternionf {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0,
                },
            })
            .size(xr::Extent2Df {
                width: 0.5,
                height: 0.5,
            });

        match frame_stream.end(
            frame_state.predicted_display_time,
            xr::EnvironmentBlendMode::OPAQUE,
            &[&proj_layer, &quad_layer],
        ) {
            Ok(_) => submitted += 1,
            Err(e) => {
                endframe_errors += 1;
                if endframe_errors <= 3 {
                    println!("  xrEndFrame error: {:?}", e);
                }
            }
        }
    }

    println!(
        "Summary: submitted={} endframe_errors={} final_state={:?}",
        submitted, endframe_errors, session_state
    );

    let _ = session.request_exit();
    Ok(())
}
