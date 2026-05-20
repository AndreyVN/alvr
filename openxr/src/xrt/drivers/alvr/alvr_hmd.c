// Copyright 2026, alvr-org.
// SPDX-License-Identifier: BSL-1.0
/*!
 * @file
 * @brief  xrt_device implementation for the ALVR-streamed HMD.
 * @ingroup drv_alvr
 *
 * Scaffolding stage: device geometry is hard-coded to a reasonable Quest 2-ish
 * default; tracking calls return XRT_ERROR_NOT_IMPLEMENTED via the bridge.
 * Phase 3 of openxr-migration.md fills these in from `alvr_packets`.
 */

#include "alvr_internal.h"

#include "math/m_api.h"
#include "math/m_space.h"
#include "util/u_device.h"
#include "util/u_misc.h"

#include <stdio.h>
#include <string.h>


static void
alvr_hmd_destroy(struct xrt_device *xdev)
{
	struct alvr_hmd *hmd = (struct alvr_hmd *)xdev;
	ALVR_DEBUG("Destroying ALVR HMD");
	u_device_free(&hmd->base);
}

static xrt_result_t
alvr_hmd_update_inputs(struct xrt_device *xdev)
{
	(void)xdev;
	return XRT_SUCCESS;
}

static xrt_result_t
alvr_hmd_get_tracked_pose(struct xrt_device *xdev,
                          enum xrt_input_name name,
                          int64_t at_timestamp_ns,
                          struct xrt_space_relation *out_relation)
{
	(void)xdev;
	(void)name;

	AlvrOxrPose pose = {0};
	AlvrOxrResult br = alvr_oxr_get_head_pose(at_timestamp_ns, &pose);
	if (br != ALVR_OXR_OK) {
		// Fall back to identity so the compositor doesn't crash during scaffolding.
		U_ZERO(out_relation);
		out_relation->relation_flags = XRT_SPACE_RELATION_BITMASK_NONE;
		return XRT_SUCCESS;
	}

	out_relation->pose.position.x = pose.position[0];
	out_relation->pose.position.y = pose.position[1];
	out_relation->pose.position.z = pose.position[2];
	out_relation->pose.orientation.x = pose.orientation[0];
	out_relation->pose.orientation.y = pose.orientation[1];
	out_relation->pose.orientation.z = pose.orientation[2];
	out_relation->pose.orientation.w = pose.orientation[3];
	out_relation->relation_flags =
	    XRT_SPACE_RELATION_ORIENTATION_VALID_BIT | XRT_SPACE_RELATION_POSITION_VALID_BIT |
	    XRT_SPACE_RELATION_ORIENTATION_TRACKED_BIT | XRT_SPACE_RELATION_POSITION_TRACKED_BIT;

	return XRT_SUCCESS;
}

static xrt_result_t
alvr_hmd_get_view_poses(struct xrt_device *xdev,
                        const struct xrt_vec3 *default_eye_relation,
                        int64_t at_timestamp_ns,
                        uint32_t view_count,
                        struct xrt_space_relation *out_head_relation,
                        struct xrt_fov *out_fovs,
                        struct xrt_pose *out_poses)
{
	(void)xdev;
	// Default impl in u_device covers identity views — good enough for scaffolding.
	u_device_get_view_poses(xdev, default_eye_relation, at_timestamp_ns, view_count,
	                        out_head_relation, out_fovs, out_poses);
	return XRT_SUCCESS;
}

struct xrt_device *
alvr_hmd_create(void)
{
	enum u_device_alloc_flags flags = U_DEVICE_ALLOC_HMD | U_DEVICE_ALLOC_TRACKING_NONE;
	struct alvr_hmd *hmd = U_DEVICE_ALLOCATE(struct alvr_hmd, flags, 1, 0);
	if (hmd == NULL) {
		return NULL;
	}

	hmd->base.name = XRT_DEVICE_GENERIC_HMD;
	hmd->base.device_type = XRT_DEVICE_TYPE_HMD;
	hmd->base.update_inputs = alvr_hmd_update_inputs;
	hmd->base.get_tracked_pose = alvr_hmd_get_tracked_pose;
	hmd->base.get_view_poses = alvr_hmd_get_view_poses;
	hmd->base.destroy = alvr_hmd_destroy;

	hmd->base.inputs[0].name = XRT_INPUT_GENERIC_HEAD_POSE;

	snprintf(hmd->base.str, XRT_DEVICE_NAME_LEN, "ALVR Streamed HMD");
	snprintf(hmd->base.serial, XRT_DEVICE_NAME_LEN, "ALVR-HMD");

	// Query the bridge for the real serial when available (no-op during scaffolding).
	alvr_oxr_get_hmd_info(hmd->base.serial, XRT_DEVICE_NAME_LEN);

	// Sensible default geometry (Quest 2-ish). Replaced by stream metadata in Phase 3.
	struct u_extents_2d extents = {.w_pixels = 2064 * 2, .h_pixels = 2208};
	u_device_setup_split_side_by_side(&hmd->base, &extents);
	hmd->base.hmd->screens[0].nominal_frame_interval_ns = (int64_t)(1.0e9 / 72.0);

	hmd->base.supported.orientation_tracking = true;
	hmd->base.supported.position_tracking = true;

	return &hmd->base;
}
