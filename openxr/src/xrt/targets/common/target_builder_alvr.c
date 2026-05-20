// Copyright 2026, alvr-org.
// SPDX-License-Identifier: BSL-1.0
/*!
 * @file
 * @brief  Target builder that wires the ALVR driver into Monado's prober.
 * @ingroup drv_alvr
 *
 * Built only when XRT_BUILD_DRIVER_ALVR is on. Inserts itself near the top of
 * target_builder_list[] (after qwerty / remote) so it overrides hardware drivers
 * when a stream is being received.
 */

#include "xrt/xrt_config_drivers.h"
#include "xrt/xrt_prober.h"
#include "xrt/xrt_results.h"
#include "util/u_builders.h"
#include "util/u_logging.h"

#ifdef XRT_BUILD_DRIVER_ALVR
#include "alvr/alvr_interface.h"
#endif

#include "target_builder_interface.h"


static const char *driver_list[] = {
    "alvr",
};

static xrt_result_t
alvr_estimate_system(struct xrt_builder *xb,
                     cJSON *config,
                     struct xrt_prober *xp,
                     struct xrt_builder_estimate *estimate)
{
	(void)xb;
	(void)config;
	(void)xp;

	estimate->certain.head = false;
	estimate->maybe.head = true;
	estimate->maybe.left = true;
	estimate->maybe.right = true;

	return XRT_SUCCESS;
}

static xrt_result_t
alvr_open_system(struct xrt_builder *xb,
                 cJSON *config,
                 struct xrt_prober *xp,
                 struct xrt_session_event_sink *broadcast,
                 struct xrt_system_devices **out_xsysd,
                 struct xrt_space_overseer **out_xso)
{
	(void)xb;
	(void)config;
	(void)xp;

#ifdef XRT_BUILD_DRIVER_ALVR
	return alvr_create_devices(broadcast, out_xsysd, out_xso);
#else
	(void)broadcast;
	(void)out_xsysd;
	(void)out_xso;
	U_LOG_E("target_builder_alvr called but XRT_BUILD_DRIVER_ALVR is off");
	return XRT_ERROR_DEVICE_CREATION_FAILED;
#endif
}

static void
alvr_destroy(struct xrt_builder *xb)
{
	free(xb);
}

struct xrt_builder *
t_builder_alvr_create(void)
{
	struct xrt_builder *xb = U_TYPED_CALLOC(struct xrt_builder);
	xb->estimate_system = alvr_estimate_system;
	xb->open_system = alvr_open_system;
	xb->destroy = alvr_destroy;
	xb->identifier = "alvr";
	xb->name = "ALVR (streamed)";
	xb->driver_identifiers = driver_list;
	xb->driver_identifier_count = ARRAY_SIZE(driver_list);

	return xb;
}
