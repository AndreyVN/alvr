// Copyright 2026, alvr-org.
// SPDX-License-Identifier: BSL-1.0
/*!
 * @file
 * @brief  Scaffolding for the ALVR fake compositor.
 * @ingroup comp_alvr
 *
 * Real implementation will extend `comp_base` to inherit xrt_compositor_native
 * boilerplate (swapchain creation via comp_swapchain, sync helpers, etc.) and
 * override `layer_commit` to package per-layer data and call
 * alvr_oxr_submit_layers.
 *
 * For Phase 2 we ship a stub that builds and warns. Anything that actually
 * tries to create a session through this compositor will see ERROR_NOT_IMPL.
 */

#include "comp_alvr.h"

#include "xrt/xrt_results.h"
#include "util/u_logging.h"


xrt_result_t
comp_alvr_create_system_compositor(struct xrt_device *xdev, struct xrt_system_compositor **out_xsysc)
{
	(void)xdev;
	(void)out_xsysc;

	U_LOG_W("comp_alvr: scaffolding stub — frame submission is not yet wired up. "
	        "See Phase 3 of openxr-migration.md.");

	return XRT_ERROR_NOT_IMPLEMENTED;
}
