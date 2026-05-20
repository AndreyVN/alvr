// Copyright 2026, alvr-org.
// SPDX-License-Identifier: BSL-1.0
/*!
 * @file
 * @brief  xrt_device implementation for ALVR-streamed controllers.
 * @ingroup drv_alvr
 *
 * Scaffolding stage: returns identity poses and ignores haptics. Real input
 * routing and bindings live in Phase 3 of openxr-migration.md.
 */

#include "alvr_internal.h"

#include "util/u_device.h"
#include "util/u_misc.h"

#include <stdio.h>
#include <string.h>


static void
alvr_controller_destroy(struct xrt_device *xdev)
{
	struct alvr_controller *ctrl = (struct alvr_controller *)xdev;
	ALVR_DEBUG("Destroying ALVR controller (side=%d)", (int)ctrl->side);
	u_device_free(&ctrl->base);
}

static xrt_result_t
alvr_controller_update_inputs(struct xrt_device *xdev)
{
	(void)xdev;
	return XRT_SUCCESS;
}

static xrt_result_t
alvr_controller_get_tracked_pose(struct xrt_device *xdev,
                                 enum xrt_input_name name,
                                 int64_t at_timestamp_ns,
                                 struct xrt_space_relation *out_relation)
{
	struct alvr_controller *ctrl = (struct alvr_controller *)xdev;
	(void)name;

	AlvrOxrControllerState state = {0};
	(void)alvr_oxr_get_controller_state(ctrl->side, at_timestamp_ns, &state);

	out_relation->pose.position.x = state.pose.position[0];
	out_relation->pose.position.y = state.pose.position[1];
	out_relation->pose.position.z = state.pose.position[2];
	out_relation->pose.orientation.x = state.pose.orientation[0];
	out_relation->pose.orientation.y = state.pose.orientation[1];
	out_relation->pose.orientation.z = state.pose.orientation[2];
	out_relation->pose.orientation.w = state.pose.orientation[3];
	out_relation->relation_flags = XRT_SPACE_RELATION_BITMASK_NONE; // until Phase 3

	return XRT_SUCCESS;
}

static xrt_result_t
alvr_controller_set_output(struct xrt_device *xdev,
                           enum xrt_output_name name,
                           const struct xrt_output_value *value)
{
	struct alvr_controller *ctrl = (struct alvr_controller *)xdev;
	(void)name;

	AlvrOxrHaptic params = {
	    .duration_ns = value->vibration.duration_ns,
	    .frequency_hz = value->vibration.frequency,
	    .amplitude = value->vibration.amplitude,
	};
	(void)alvr_oxr_set_haptic(ctrl->side, &params);
	return XRT_SUCCESS;
}

struct xrt_device *
alvr_controller_create(AlvrOxrSide side)
{
	enum u_device_alloc_flags flags = U_DEVICE_ALLOC_TRACKING_NONE;
	struct alvr_controller *ctrl = U_DEVICE_ALLOCATE(struct alvr_controller, flags, 1, 1);
	if (ctrl == NULL) {
		return NULL;
	}

	ctrl->side = side;
	ctrl->base.name = XRT_DEVICE_TOUCH_CONTROLLER;
	ctrl->base.device_type = (side == ALVR_OXR_SIDE_LEFT)
	                             ? XRT_DEVICE_TYPE_LEFT_HAND_CONTROLLER
	                             : XRT_DEVICE_TYPE_RIGHT_HAND_CONTROLLER;
	ctrl->base.update_inputs = alvr_controller_update_inputs;
	ctrl->base.get_tracked_pose = alvr_controller_get_tracked_pose;
	ctrl->base.set_output = alvr_controller_set_output;
	ctrl->base.destroy = alvr_controller_destroy;

	ctrl->base.inputs[0].name = XRT_INPUT_TOUCH_GRIP_POSE;
	ctrl->base.outputs[0].name = XRT_OUTPUT_NAME_TOUCH_HAPTIC;

	const char *suffix = (side == ALVR_OXR_SIDE_LEFT) ? "L" : "R";
	snprintf(ctrl->base.str, XRT_DEVICE_NAME_LEN, "ALVR Streamed Controller (%s)", suffix);
	snprintf(ctrl->base.serial, XRT_DEVICE_NAME_LEN, "ALVR-CTRL-%s", suffix);

	alvr_oxr_get_controller_info(side, ctrl->base.serial, XRT_DEVICE_NAME_LEN);

	ctrl->base.supported.orientation_tracking = true;
	ctrl->base.supported.position_tracking = true;

	return &ctrl->base;
}
