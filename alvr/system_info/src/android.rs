use alvr_common::warn;
use jni::{
    Env, JavaVM,
    errors::Result as JniResult,
    jni_sig, jni_str,
    objects::{JIntArray, JObject, JString},
    refs::Reference,
    strings::JNIStr,
    sys::jobject,
};
use std::{
    ffi::CStr,
    fs,
    net::{IpAddr, Ipv4Addr},
    path::Path,
};

pub const MICROPHONE_PERMISSION: &str = "android.permission.RECORD_AUDIO";

pub fn vm() -> JavaVM {
    unsafe { JavaVM::from_raw(ndk_context::android_context().vm().cast()) }
}

pub fn context() -> jobject {
    ndk_context::android_context().context().cast()
}

fn get_api_level() -> i32 {
    vm().attach_current_thread(|env| {
        env.get_static_field(
            jni_str!("android/os/Build$VERSION"),
            jni_str!("SDK_INT"),
            jni_sig!("I"),
        )?
        .i()
    })
    .unwrap()
}

pub fn try_get_permission(permission: &str) {
    vm().attach_current_thread(|env| {
        let mic_perm_jstring = env.new_string(permission)?;

        let permission_status = env
            .call_method(
                unsafe { JObject::global_kind_from_raw(context()) },
                jni_str!("checkSelfPermission"),
                jni_sig!("(Ljava/lang/String;)I"),
                &[(&mic_perm_jstring).into()],
            )?
            .i()?;

        if permission_status != 0 {
            let perm_array =
                env.new_object_array(1, jni_str!("java/lang/String"), mic_perm_jstring)?;

            env.call_method(
                unsafe { JObject::global_kind_from_raw(context()) },
                jni_str!("requestPermissions"),
                jni_sig!("([Ljava/lang/String;I)V"),
                &[(&perm_array).into(), 0.into()],
            )?;
            // todo: handle case where permission is rejected
        }

        JniResult::Ok(())
    })
    .unwrap();
}

pub fn build_string(ty: &CStr) -> String {
    vm().attach_current_thread(|env| {
        let jname = env
            .get_static_field(
                jni_str!("android/os/Build"),
                JNIStr::from_cstr(ty).unwrap(),
                jni_sig!("Ljava/lang/String;"),
            )?
            .l()?;
        JniResult::Ok(env.cast_local::<JString>(jname)?.to_string())
    })
    .unwrap()
}

pub fn device_name() -> String {
    build_string(c"DEVICE")
}

pub fn model_name() -> String {
    build_string(c"MODEL")
}

pub fn manufacturer_name() -> String {
    build_string(c"MANUFACTURER")
}

pub fn product_name() -> String {
    build_string(c"PRODUCT")
}

fn get_system_service<'a>(env: &mut Env<'a>, service_name: &str) -> JniResult<JObject<'a>> {
    let service_str = env.new_string(service_name)?;

    env.call_method(
        unsafe { JObject::global_kind_from_raw(context()) },
        jni_str!("getSystemService"),
        jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
        &[(&service_str).into()],
    )?
    .l()
}

// Note: tried and failed to use libc
pub fn local_ip() -> IpAddr {
    vm().attach_current_thread(|env| {
        let wifi_manager = get_system_service(env, "wifi")?;
        let wifi_info = env
            .call_method(
                wifi_manager,
                jni_str!("getConnectionInfo"),
                jni_sig!("()Landroid/net/wifi/WifiInfo;"),
                &[],
            )?
            .l()?;
        let ip_i32 = env
            .call_method(wifi_info, jni_str!("getIpAddress"), jni_sig!("()I"), &[])?
            .i()?;

        let ip_arr = ip_i32.to_le_bytes();

        JniResult::Ok(IpAddr::V4(Ipv4Addr::new(
            ip_arr[0], ip_arr[1], ip_arr[2], ip_arr[3],
        )))
    })
    .unwrap()
}

// This is needed to avoid wifi scans that disrupt streaming.
// Code inspired from https://github.com/Meumeu/WiVRn/blob/master/client/application.cpp
pub fn set_wifi_lock(enabled: bool) {
    vm().attach_current_thread(|env| {
        let wifi_manager = get_system_service(env, "wifi")?;

        fn set_lock<'a>(env: &mut Env<'a>, lock: &JObject, enabled: bool) -> JniResult<()> {
            env.call_method(
                lock,
                jni_str!("setReferenceCounted"),
                jni_sig!("(Z)V"),
                &[false.into()],
            )?;
            env.call_method(
                &lock,
                if enabled {
                    jni_str!("acquire")
                } else {
                    jni_str!("release")
                },
                jni_sig!("()V"),
                &[],
            )?;

            let lock_is_aquired = env
                .call_method(lock, jni_str!("isHeld"), jni_sig!("()Z"), &[])?
                .z()?;

            if lock_is_aquired != enabled {
                warn!("Failed to set wifi lock: expected {enabled}, got {lock_is_aquired}");
            }

            JniResult::Ok(())
        }

        let wifi_lock_jstring = env.new_string("alvr_wifi_lock")?;
        let wifi_lock = env
            .call_method(
                &wifi_manager,
                jni_str!("createWifiLock"),
                jni_sig!("(ILjava/lang/String;)Landroid/net/wifi/WifiManager$WifiLock;"),
                &[
                    if get_api_level() >= 29 {
                        // Recommended for virtual reality since it disables WIFI scans
                        4 // WIFI_MODE_FULL_LOW_LATENCY
                    } else {
                        3 // WIFI_MODE_FULL_HIGH_PERF
                    }
                    .into(),
                    (&wifi_lock_jstring).into(),
                ],
            )?
            .l()?;
        set_lock(env, &wifi_lock, enabled)?;

        let multicast_lock_jstring = env.new_string("alvr_multicast_lock")?;
        let multicast_lock = env
            .call_method(
                wifi_manager,
                jni_str!("createMulticastLock"),
                jni_sig!("(Ljava/lang/String;)Landroid/net/wifi/WifiManager$MulticastLock;"),
                &[(&multicast_lock_jstring).into()],
            )?
            .l()?;
        set_lock(env, &multicast_lock, enabled)?;

        JniResult::Ok(())
    })
    .unwrap();
}

#[derive(Debug, Clone, Copy)]
pub struct ControllerBatteryStatus {
    pub is_left: bool,
    pub gauge_value: f32,
    pub is_plugged: bool,
}

// Reads battery from one Android InputDevice. Returns None when the device is not a controller,
// has no battery state, or its hand cannot be inferred from the name.
fn read_controller_battery<'a>(
    env: &mut Env<'a>,
    input_manager: &JObject,
    id: i32,
) -> JniResult<Option<ControllerBatteryStatus>> {
    // Android InputDevice source bits (frameworks/base/core/java/android/view/InputDevice.java).
    const SOURCE_JOYSTICK: i32 = 0x0100_0010;
    const SOURCE_GAMEPAD: i32 = 0x0000_0401;
    // BatteryManager.BATTERY_STATUS_CHARGING / _FULL.
    const BATTERY_STATUS_CHARGING: i32 = 2;
    const BATTERY_STATUS_FULL: i32 = 5;

    let device = env
        .call_method(
            input_manager,
            jni_str!("getInputDevice"),
            jni_sig!("(I)Landroid/view/InputDevice;"),
            &[id.into()],
        )?
        .l()?;
    if device.is_null() {
        return Ok(None);
    }

    let sources = env
        .call_method(&device, jni_str!("getSources"), jni_sig!("()I"), &[])?
        .i()?;
    let is_controller = (sources & SOURCE_GAMEPAD) == SOURCE_GAMEPAD
        || (sources & SOURCE_JOYSTICK) == SOURCE_JOYSTICK;
    if !is_controller {
        return Ok(None);
    }

    let battery_state = env
        .call_method(
            &device,
            jni_str!("getBatteryState"),
            jni_sig!("()Landroid/view/InputDevice$BatteryState;"),
            &[],
        )?
        .l()?;
    if battery_state.is_null() {
        return Ok(None);
    }

    let is_present = env
        .call_method(&battery_state, jni_str!("isPresent"), jni_sig!("()Z"), &[])?
        .z()?;
    if !is_present {
        return Ok(None);
    }

    let capacity = env
        .call_method(
            &battery_state,
            jni_str!("getCapacity"),
            jni_sig!("()F"),
            &[],
        )?
        .f()?;
    if !capacity.is_finite() {
        return Ok(None);
    }

    let status = env
        .call_method(&battery_state, jni_str!("getStatus"), jni_sig!("()I"), &[])?
        .i()?;
    let is_plugged = status == BATTERY_STATUS_CHARGING || status == BATTERY_STATUS_FULL;

    let name_obj = env
        .call_method(
            &device,
            jni_str!("getName"),
            jni_sig!("()Ljava/lang/String;"),
            &[],
        )?
        .l()?;
    if name_obj.is_null() {
        return Ok(None);
    }
    let name = env.cast_local::<JString>(name_obj)?.to_string();
    let lower = name.to_ascii_lowercase();

    let is_left = lower.contains("left");
    let is_right = lower.contains("right");
    if is_left == is_right {
        return Ok(None);
    }

    Ok(Some(ControllerBatteryStatus {
        is_left,
        gauge_value: capacity,
        is_plugged,
    }))
}

/// Enumerates Android InputDevices, finds VR controllers via `SOURCE_GAMEPAD`/`SOURCE_JOYSTICK`,
/// and returns the battery capacity from `InputDevice.getBatteryState()` (API 29+).
/// Returns an empty Vec on older devices or when no matching controllers are found.
pub fn get_controller_battery_status() -> Vec<ControllerBatteryStatus> {
    if get_api_level() < 29 {
        return Vec::new();
    }

    vm().attach_current_thread(|env| -> JniResult<Vec<ControllerBatteryStatus>> {
        let input_manager = get_system_service(env, "input")?;

        let ids_obj = env
            .call_method(
                &input_manager,
                jni_str!("getInputDeviceIds"),
                jni_sig!("()[I"),
                &[],
            )?
            .l()?;
        if ids_obj.is_null() {
            return Ok(Vec::new());
        }
        let ids_array = env.cast_local::<JIntArray>(ids_obj)?;
        let len = ids_array.len(env)?;
        let mut ids = vec![0i32; len];
        ids_array.get_region(env, 0, &mut ids)?;

        let mut out = Vec::new();
        for id in ids {
            if let Ok(Some(status)) = read_controller_battery(env, &input_manager, id) {
                out.push(status);
            }
        }
        Ok(out)
    })
    .unwrap_or_else(|e: jni::errors::Error| {
        warn!("get_controller_battery_status failed: {e}");
        Vec::new()
    })
}

pub fn get_battery_status() -> (f32, bool) {
    vm().attach_current_thread(|env| {
        let intent_action_jstring = env.new_string("android.intent.action.BATTERY_CHANGED")?;
        let intent_filter = env.new_object(
            jni_str!("android/content/IntentFilter"),
            jni_sig!("(Ljava/lang/String;)V"),
            &[(&intent_action_jstring).into()],
        )?;
        let battery_intent = env
            .call_method(
                unsafe { JObject::global_kind_from_raw(context()) },
                jni_str!("registerReceiver"),
                jni_sig!(
                    "(Landroid/content/BroadcastReceiver;Landroid/content/IntentFilter;)Landroid/content/Intent;"
                ),
                &[(&JObject::null()).into(), (&intent_filter).into()],
            )?
            .l()?;

        fn get_battery_value<'a>(env: &mut Env<'a>, battery_intent: &JObject, key: &str) -> JniResult<i32> {
            let key_jstring = env.new_string(key)?;
            env.call_method(
                battery_intent,
                jni_str!("getIntExtra"),
                jni_sig!("(Ljava/lang/String;I)I"),
                &[(&key_jstring).into(), (-1).into()],
            )?
            .i()
        }

        let level = get_battery_value(env, &battery_intent, "level")?;
        let scale = get_battery_value(env, &battery_intent, "scale")?;
        let plugged = get_battery_value(env, &battery_intent, "plugged")?;

        JniResult::Ok((level as f32 / scale as f32, plugged > 0))
    })
    .unwrap()
}

/// Battery sensor temperature in degrees Celsius. Reads `BatteryManager.EXTRA_TEMPERATURE` (tenths
/// of a degree) from the same sticky intent as `get_battery_status`. Returns `None` if the field is
/// missing.
pub fn get_battery_temperature_c() -> Option<f32> {
    vm().attach_current_thread(|env| {
        let intent_action_jstring = env.new_string("android.intent.action.BATTERY_CHANGED")?;
        let intent_filter = env.new_object(
            jni_str!("android/content/IntentFilter"),
            jni_sig!("(Ljava/lang/String;)V"),
            &[(&intent_action_jstring).into()],
        )?;
        let battery_intent = env
            .call_method(
                unsafe { JObject::global_kind_from_raw(context()) },
                jni_str!("registerReceiver"),
                jni_sig!(
                    "(Landroid/content/BroadcastReceiver;Landroid/content/IntentFilter;)Landroid/content/Intent;"
                ),
                &[(&JObject::null()).into(), (&intent_filter).into()],
            )?
            .l()?;

        let key_jstring = env.new_string("temperature")?;
        let tenths = env
            .call_method(
                &battery_intent,
                jni_str!("getIntExtra"),
                jni_sig!("(Ljava/lang/String;I)I"),
                &[(&key_jstring).into(), i32::MIN.into()],
            )?
            .i()?;

        JniResult::Ok(if tenths == i32::MIN {
            None
        } else {
            Some(tenths as f32 / 10.0)
        })
    })
    .unwrap_or(None)
}

/// Android `PowerManager` thermal signals. Returns `(thermal_status, thermal_headroom)`:
/// - `thermal_status`: discrete bucket from `getCurrentThermalStatus()` (API 29+).
///   0=NONE, 1=LIGHT, 2=MODERATE, 3=SEVERE, 4=CRITICAL, 5=EMERGENCY, 6=SHUTDOWN.
/// - `thermal_headroom`: `getThermalHeadroom(0)` forecast (API 30+). 0.0..1.0+, 1.0 ≈ imminent
///   throttling.
///
/// Either field can be `None` independently (e.g. API 29 has status but not headroom).
pub fn get_thermal_state() -> (Option<i32>, Option<f32>) {
    let api = get_api_level();
    if api < 29 {
        return (None, None);
    }

    vm().attach_current_thread(|env| -> JniResult<(Option<i32>, Option<f32>)> {
        let power_manager = get_system_service(env, "power")?;
        if power_manager.is_null() {
            return Ok((None, None));
        }

        let status = env
            .call_method(
                &power_manager,
                jni_str!("getCurrentThermalStatus"),
                jni_sig!("()I"),
                &[],
            )
            .ok()
            .and_then(|v| v.i().ok());

        let headroom = if api >= 30 {
            env.call_method(
                &power_manager,
                jni_str!("getThermalHeadroom"),
                jni_sig!("(I)F"),
                &[0.into()],
            )
            .ok()
            .and_then(|v| v.f().ok())
            .filter(|h| h.is_finite())
        } else {
            None
        };

        Ok((status, headroom))
    })
    .unwrap_or((None, None))
}

/// Snapshot of `/proc/meminfo` plus the client process RSS. All values in kibibytes.
#[derive(Debug, Clone, Copy, Default)]
pub struct MemInfo {
    pub total_kib: Option<u64>,
    pub available_kib: Option<u64>,
    pub process_rss_kib: Option<u64>,
}

pub fn get_meminfo() -> MemInfo {
    let mut info = MemInfo::default();

    if let Ok(content) = fs::read_to_string("/proc/meminfo") {
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                info.total_kib = parse_kib(rest);
            } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
                info.available_kib = parse_kib(rest);
            }
        }
    }

    if let Ok(content) = fs::read_to_string("/proc/self/status") {
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                info.process_rss_kib = parse_kib(rest);
                break;
            }
        }
    }

    info
}

fn parse_kib(value: &str) -> Option<u64> {
    // Format: "<whitespace>123456 kB".
    let mut parts = value.split_whitespace();
    let n: u64 = parts.next()?.parse().ok()?;
    Some(n)
}

/// Aggregate CPU jiffies from the first line of `/proc/stat` plus per-process utime+stime from
/// `/proc/self/stat`. Used by [`CpuSampler`] to compute deltas.
#[derive(Debug, Clone, Copy, Default)]
struct CpuStat {
    total: u64,
    busy: u64,
    process: u64,
}

fn read_cpu_stat() -> Option<CpuStat> {
    let stat = fs::read_to_string("/proc/stat").ok()?;
    let first_line = stat.lines().next()?;
    let rest = first_line.strip_prefix("cpu")?.trim_start();
    let fields: Vec<u64> = rest
        .split_whitespace()
        .take(8)
        .map(|s| s.parse::<u64>().unwrap_or(0))
        .collect();
    if fields.len() < 4 {
        return None;
    }
    // user, nice, system, idle, iowait, irq, softirq, steal
    let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
    let total: u64 = fields.iter().sum();
    let busy = total.saturating_sub(idle);

    let proc_stat = fs::read_to_string("/proc/self/stat").ok()?;
    // After "(comm)" we have one-char state then numeric fields. utime is field 14, stime is 15
    // (1-indexed); but the (comm) field can contain spaces so we split off everything past the
    // final ')' to avoid that minefield.
    let after_comm = proc_stat.rsplit_once(')').map(|(_, rest)| rest)?;
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    // Index into the slice that begins after ')': the first entry there is field 3 (state).
    // utime is field 14 → index 11; stime is field 15 → index 12.
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;

    Some(CpuStat {
        total,
        busy,
        process: utime + stime,
    })
}

/// Two-sample CPU utilization estimator. Keeps the previous jiffy counters and exposes a `sample()`
/// method that returns aggregate + per-process busy fractions in [0, 1].
#[derive(Default)]
pub struct CpuSampler {
    prev: Option<CpuStat>,
}

impl CpuSampler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `(system_busy_fraction, process_busy_fraction)` since the previous call. Either can
    /// be `None` if procfs is unreadable or the delta window has zero total time. The first call
    /// after construction primes the sampler and returns `(None, None)`.
    pub fn sample(&mut self) -> (Option<f32>, Option<f32>) {
        let Some(now) = read_cpu_stat() else {
            return (None, None);
        };
        let Some(prev) = self.prev.replace(now) else {
            return (None, None);
        };
        let total_delta = now.total.saturating_sub(prev.total);
        if total_delta == 0 {
            return (None, None);
        }
        let busy_delta = now.busy.saturating_sub(prev.busy);
        let proc_delta = now.process.saturating_sub(prev.process);
        let total_fraction = busy_delta as f32 / total_delta as f32;
        let process_fraction = proc_delta as f32 / total_delta as f32;
        (Some(total_fraction), Some(process_fraction))
    }
}

/// KGSL GPU counters from Adreno's `/sys/class/kgsl/kgsl-3d0` (Snapdragon). Some firmwares restrict
/// these sysfs entries; in that case all reads return `None` and the sampler degrades gracefully.
#[derive(Default)]
pub struct GpuSampler {
    prev_busy: Option<u64>,
    prev_total: Option<u64>,
}

impl GpuSampler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `(busy_fraction, freq_hz)`. The first call primes the busy counters and returns
    /// `(None, freq_hz)`. Subsequent calls return the busy fraction between samples.
    pub fn sample(&mut self) -> (Option<f32>, Option<u64>) {
        let freq_hz = read_kgsl_freq_hz();
        let Some((busy, total)) = read_kgsl_gpubusy() else {
            return (None, freq_hz);
        };
        let busy_fraction = match (self.prev_busy.replace(busy), self.prev_total.replace(total)) {
            (Some(prev_busy), Some(prev_total)) => {
                let busy_delta = busy.saturating_sub(prev_busy);
                let total_delta = total.saturating_sub(prev_total);
                (total_delta > 0).then(|| busy_delta as f32 / total_delta as f32)
            }
            _ => None,
        };
        (busy_fraction, freq_hz)
    }
}

fn read_kgsl_gpubusy() -> Option<(u64, u64)> {
    // Two whitespace-separated ints: busy_ticks total_ticks.
    let raw = fs::read_to_string("/sys/class/kgsl/kgsl-3d0/gpubusy").ok()?;
    let mut parts = raw.split_whitespace();
    let busy: u64 = parts.next()?.parse().ok()?;
    let total: u64 = parts.next()?.parse().ok()?;
    Some((busy, total))
}

fn read_kgsl_freq_hz() -> Option<u64> {
    // Try devfreq first (more accurate on recent SoCs), fall back to legacy `gpuclk`.
    let devfreq_path = Path::new("/sys/class/kgsl/kgsl-3d0/devfreq/cur_freq");
    let raw = fs::read_to_string(devfreq_path)
        .or_else(|_| fs::read_to_string("/sys/class/kgsl/kgsl-3d0/gpuclk"))
        .ok()?;
    raw.trim().parse().ok()
}
