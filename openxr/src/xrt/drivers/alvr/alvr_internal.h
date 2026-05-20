// Copyright 2026, alvr-org.
// SPDX-License-Identifier: BSL-1.0
/*!
 * @file
 * @brief  Internal types shared by the ALVR driver files.
 * @ingroup drv_alvr
 */

#pragma once

#include "xrt/xrt_device.h"
#include "util/u_logging.h"

#include "alvr_runtime_bridge.h" // generated/maintained in alvr/server_openxr/include

#ifdef __cplusplus
extern "C" {
#endif

/*!
 * Driver-wide logging level (controlled by env ALVR_LOG, default INFO).
 */
enum u_logging_level
alvr_log_level(void);

#define ALVR_TRACE(...) U_LOG_IFL_T(alvr_log_level(), __VA_ARGS__)
#define ALVR_DEBUG(...) U_LOG_IFL_D(alvr_log_level(), __VA_ARGS__)
#define ALVR_INFO(...)  U_LOG_IFL_I(alvr_log_level(), __VA_ARGS__)
#define ALVR_WARN(...)  U_LOG_IFL_W(alvr_log_level(), __VA_ARGS__)
#define ALVR_ERROR(...) U_LOG_IFL_E(alvr_log_level(), __VA_ARGS__)

/*!
 * Concrete xrt_device implementation for the ALVR-streamed HMD.
 *
 * @extends xrt_device
 */
struct alvr_hmd
{
	struct xrt_device base;
};

/*!
 * Concrete xrt_device implementation for one ALVR-streamed controller.
 *
 * @extends xrt_device
 */
struct alvr_controller
{
	struct xrt_device base;
	AlvrOxrSide side;
};

struct xrt_device *
alvr_hmd_create(void);

struct xrt_device *
alvr_controller_create(AlvrOxrSide side);

#ifdef __cplusplus
}
#endif
