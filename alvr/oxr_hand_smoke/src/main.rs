use openxr as xr;
use std::{
    error::Error,
    thread,
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn Error>> {
    println!("=== oxr_hand_smoke ===");
    println!(
        "XR_RUNTIME_JSON = {:?}",
        std::env::var("XR_RUNTIME_JSON").ok()
    );

    let entry = unsafe { xr::Entry::load()? };

    let available = entry.enumerate_extensions()?;
    println!("XR_MND_headless available    = {}", available.mnd_headless);
    println!(
        "XR_EXT_hand_tracking available = {}",
        available.ext_hand_tracking
    );
    if !available.mnd_headless || !available.ext_hand_tracking {
        return Err("required extension missing".into());
    }

    let mut enabled = xr::ExtensionSet::default();
    enabled.mnd_headless = true;
    enabled.ext_hand_tracking = true;
    #[cfg(target_os = "windows")]
    {
        enabled.khr_win32_convert_performance_counter_time =
            available.khr_win32_convert_performance_counter_time;
    }

    let instance = entry.create_instance(
        &xr::ApplicationInfo {
            application_name: "oxr_hand_smoke",
            application_version: 1,
            engine_name: "alvr",
            engine_version: 0,
            api_version: xr::Version::new(1, 0, 34),
        },
        &enabled,
        &[],
    )?;
    let ip = instance.properties()?;
    println!("Runtime: {} v{}", ip.runtime_name, ip.runtime_version);

    let system = instance.system(xr::FormFactor::HEAD_MOUNTED_DISPLAY)?;
    let sp = instance.system_properties(system)?;
    println!(
        "System : id={} name={:?} vendor=0x{:x}",
        sp.system_id.into_raw(),
        sp.system_name,
        sp.vendor_id
    );

    let (session, _frame_waiter, _frame_stream) = unsafe {
        instance.create_session::<xr::Headless>(system, &xr::headless::SessionCreateInfo {})?
    };
    println!("Headless session created");

    session.begin(xr::ViewConfigurationType::PRIMARY_STEREO)?;
    println!("session.begin() ok");

    // Drain session events until we reach FOCUSED (or time out).
    let mut storage = xr::EventDataBuffer::new();
    let drain_start = Instant::now();
    let mut last_state = xr::SessionState::UNKNOWN;
    while drain_start.elapsed() < Duration::from_secs(5) {
        while let Some(ev) = instance.poll_event(&mut storage)? {
            if let xr::Event::SessionStateChanged(e) = ev {
                last_state = e.state();
                println!("  session state -> {:?}", last_state);
            }
        }
        if last_state == xr::SessionState::FOCUSED
            || last_state == xr::SessionState::SYNCHRONIZED
            || last_state == xr::SessionState::VISIBLE
        {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }

    let left_tracker = session.create_hand_tracker(xr::Hand::LEFT)?;
    let right_tracker = session.create_hand_tracker(xr::Hand::RIGHT)?;
    println!("HandTracker (L+R) created");

    let spaces = session.enumerate_reference_spaces()?;
    println!("Supported reference spaces: {:?}", spaces);
    let space_type = if spaces.contains(&xr::ReferenceSpaceType::STAGE) {
        xr::ReferenceSpaceType::STAGE
    } else if spaces.contains(&xr::ReferenceSpaceType::LOCAL) {
        xr::ReferenceSpaceType::LOCAL
    } else {
        xr::ReferenceSpaceType::VIEW
    };
    println!("Using reference space: {:?}", space_type);
    let stage = session.create_reference_space(space_type, xr::Posef::IDENTITY)?;

    // Probe xrLocateViews — useful for cross-checking against UE5 / D3D12-binding
    // OpenXR clients that fail at this call. Headless session has no graphics
    // binding, so we're testing Monado's view-location path in isolation.
    println!("Probing xrLocateViews (PRIMARY_STEREO)...");
    let probe_now = instance.now()?;
    match session.locate_views(xr::ViewConfigurationType::PRIMARY_STEREO, probe_now, &stage) {
        Ok((flags, views)) => {
            println!(
                "  locate_views OK: flags=0x{:x} view_count={}",
                flags.into_raw(),
                views.len()
            );
            for (i, v) in views.iter().enumerate() {
                let p = v.pose.position;
                let f = v.fov;
                println!(
                    "  view[{i}] pos=({:+.3},{:+.3},{:+.3}) fov={{l={:+.3} r={:+.3} u={:+.3} d={:+.3}}}",
                    p.x, p.y, p.z, f.angle_left, f.angle_right, f.angle_up, f.angle_down
                );
            }
        }
        Err(e) => {
            println!("  locate_views ERR: {e:?}");
        }
    }

    println!("Sampling 25 joints/hand for 8s (printing every ~0.5s)...");
    let start = Instant::now();
    let mut frame = 0u32;
    let mut last_print = Instant::now();
    let mut l_valid = 0u32;
    let mut r_valid = 0u32;
    while start.elapsed() < Duration::from_secs(8) {
        // Drain events so the session can keep progressing.
        while let Some(_ev) = instance.poll_event(&mut storage)? {}

        let now = instance.now()?;

        let l = stage.locate_hand_joints(&left_tracker, now).ok().flatten();
        let r = stage.locate_hand_joints(&right_tracker, now).ok().flatten();

        if let Some(j) = &l
            && j[0]
                .location_flags
                .contains(xr::SpaceLocationFlags::POSITION_VALID)
        {
            l_valid += 1;
        }
        if let Some(j) = &r
            && j[0]
                .location_flags
                .contains(xr::SpaceLocationFlags::POSITION_VALID)
        {
            r_valid += 1;
        }

        if last_print.elapsed() >= Duration::from_millis(500) {
            last_print = Instant::now();
            print_hand("L", &l);
            print_hand("R", &r);
        }
        frame += 1;
        thread::sleep(Duration::from_millis(16));
    }

    println!(
        "Summary: frames={} L valid={} ({}%) R valid={} ({}%)",
        frame,
        l_valid,
        l_valid * 100 / frame.max(1),
        r_valid,
        r_valid * 100 / frame.max(1)
    );

    session.request_exit().ok();
    Ok(())
}

fn print_hand(label: &str, joints: &Option<xr::HandJointLocations>) {
    match joints {
        None => println!("  {} no data (no joints returned)", label),
        Some(j) => {
            let wrist = &j[0];
            let flags = wrist.location_flags;
            let p = wrist.pose.position;
            let q = wrist.pose.orientation;
            println!(
                "  {} wrist pos=({:+.3},{:+.3},{:+.3}) ori=({:+.2},{:+.2},{:+.2},{:+.2}) flags=0x{:x}",
                label,
                p.x,
                p.y,
                p.z,
                q.x,
                q.y,
                q.z,
                q.w,
                flags.into_raw()
            );
        }
    }
}
