// Copyright 2026, alvr-org.
// SPDX-License-Identifier: BSL-1.0
/*!
 * @file
 * @brief  ALVR driver entry point; creates the streamed HMD + controllers.
 * @ingroup drv_alvr
 */

#include "alvr_interface.h"
#include "alvr_internal.h"

#include "xrt/xrt_device.h"
#include "xrt/xrt_results.h"
#include "xrt/xrt_system.h"
#include "util/u_debug.h"
#include "util/u_misc.h"
#include "util/u_system_helpers.h"
#include "util/u_space_overseer.h"

#include <stdlib.h>


DEBUG_GET_ONCE_LOG_OPTION(alvr_log, "ALVR_LOG", U_LOGGING_INFO)

enum u_logging_level
alvr_log_level(void)
{
	return debug_get_log_option_alvr_log();
}

xrt_result_t
alvr_create_devices(struct xrt_session_event_sink *broadcast,
                    struct xrt_system_devices **out_xsysd,
                    struct xrt_space_overseer **out_xso)
{
	ALVR_INFO("Creating ALVR system devices (scaffolding stage; bridge is stubbed)");

	(void)broadcast;

	if (out_xsysd == NULL || out_xso == NULL) {
		ALVR_ERROR("out_xsysd / out_xso must be non-NULL");
		return XRT_ERROR_ALLOCATION;
	}

	AlvrOxrResult init_res = alvr_oxr_init();
	if (init_res != ALVR_OXR_RESULT_OK) {
		ALVR_ERROR("alvr_oxr_init failed: %d", (int)init_res);
		return XRT_ERROR_DEVICE_CREATION_FAILED;
	}

	struct u_system_devices_static *usysds = u_system_devices_static_allocate();
	struct xrt_system_devices *xsysd = &usysds->base.base;

	struct xrt_device *hmd = alvr_hmd_create();
	if (hmd == NULL) {
		u_system_devices_destroy(&xsysd);
		alvr_oxr_shutdown();
		return XRT_ERROR_DEVICE_CREATION_FAILED;
	}
	xsysd->xdevs[xsysd->xdev_count++] = hmd;
	xsysd->static_roles.head = hmd;

	struct xrt_device *left = alvr_controller_create(ALVR_OXR_SIDE_LEFT);
	struct xrt_device *right = alvr_controller_create(ALVR_OXR_SIDE_RIGHT);

	if (left != NULL) {
		xsysd->xdevs[xsysd->xdev_count++] = left;
	}
	if (right != NULL) {
		xsysd->xdevs[xsysd->xdev_count++] = right;
	}

	*out_xsysd = xsysd;
	*out_xso = u_space_overseer_create(broadcast);

	ALVR_INFO("ALVR driver scaffolding ready (%u devices)", (unsigned)xsysd->xdev_count);
	return XRT_SUCCESS;
}
