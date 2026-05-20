// Copyright 2026, alvr-org.
// SPDX-License-Identifier: BSL-1.0
/*!
 * @file
 * @brief  Public interface for the ALVR "fake" compositor.
 * @ingroup comp_alvr
 *
 * Produces an xrt_system_compositor that accepts OpenXR layers and forwards
 * them to ALVR's encoder through alvr_oxr_submit_layers, instead of presenting
 * them to a real display.
 *
 * Built only when XRT_FEATURE_COMP_ALVR is enabled (default OFF).
 */

#pragma once

#include "xrt/xrt_compositor.h"

#ifdef __cplusplus
extern "C" {
#endif

struct xrt_device;

/*!
 * Create the ALVR system compositor.
 *
 * @param[in]  xdev      The head device (ALVR streamed HMD).
 * @param[out] out_xsysc Created system compositor.
 *
 * @ingroup comp_alvr
 */
xrt_result_t
comp_alvr_create_system_compositor(struct xrt_device *xdev, struct xrt_system_compositor **out_xsysc);

#ifdef __cplusplus
}
#endif
