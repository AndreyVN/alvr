// Copyright 2026, alvr-org.
// SPDX-License-Identifier: BSL-1.0
/*!
 * @file
 * @brief  Public interface for the ALVR Monado driver.
 * @ingroup drv_alvr
 *
 * The ALVR driver presents the streamed headset + controllers (received from
 * the Android client over UDP via `alvr/server_openxr`) as xrt_devices, so
 * Monado can expose them to any OpenXR app.
 *
 * Built only when `XRT_BUILD_DRIVER_ALVR=ON`. At this scaffolding stage all
 * device callbacks return XRT_ERROR_NOT_IMPLEMENTED — the actual streaming
 * wiring is Phase 3 of openxr-migration.md.
 */

#pragma once

#include "xrt/xrt_compiler.h"
#include "xrt/xrt_defines.h"

#ifdef __cplusplus
extern "C" {
#endif

struct xrt_session_event_sink;
struct xrt_system_devices;
struct xrt_space_overseer;

/*!
 * @defgroup drv_alvr ALVR streaming driver
 * @ingroup drv
 *
 * Devices exposed by this driver are populated from packets received by the
 * ALVR PC server (see crate `alvr_server_openxr`). The driver is the symmetric
 * counterpart of `alvr/server_openvr`, which presents the same stream as a
 * SteamVR driver.
 */

/*!
 * @dir drivers/alvr
 *
 * @brief @ref drv_alvr files.
 */

/*!
 * Create the ALVR-streamed system: one HMD + left/right controllers.
 *
 * @param[in]  broadcast Event sink used for cross-session events (e.g. focus
 *                       loss when the headset connection drops).
 * @param[out] out_xsysd System-devices set populated with the streamed devices.
 * @param[out] out_xso   Space overseer (default u_space_overseer is fine).
 *
 * @ingroup drv_alvr
 */
xrt_result_t
alvr_create_devices(struct xrt_session_event_sink *broadcast,
                    struct xrt_system_devices **out_xsysd,
                    struct xrt_space_overseer **out_xso);

#ifdef __cplusplus
}
#endif
