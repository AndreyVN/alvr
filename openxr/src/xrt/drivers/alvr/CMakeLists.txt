# Copyright 2026, alvr-org.
# SPDX-License-Identifier: BSL-1.0

if(NOT XRT_BUILD_DRIVER_ALVR)
	return()
endif()

# Path to alvr_server_openxr's bridge header. Override at configure time via
# -DALVR_BRIDGE_HEADER_DIR=... when the crate is built somewhere unusual.
if(NOT DEFINED ALVR_BRIDGE_HEADER_DIR)
	set(ALVR_BRIDGE_HEADER_DIR "${CMAKE_SOURCE_DIR}/../alvr/server_openxr/include")
endif()

if(NOT EXISTS "${ALVR_BRIDGE_HEADER_DIR}/alvr_runtime_bridge.h")
	message(
		FATAL_ERROR
		"alvr_runtime_bridge.h not found at ${ALVR_BRIDGE_HEADER_DIR}. "
		"Build the alvr_server_openxr crate first (cargo build -p alvr_server_openxr) "
		"or pass -DALVR_BRIDGE_HEADER_DIR=<dir>."
	)
endif()

add_library(drv_alvr STATIC
	alvr_interface.h
	alvr_internal.h
	alvr_hub.c
	alvr_hmd.c
	alvr_controller.c
)

target_include_directories(drv_alvr
	PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}
	PRIVATE ${ALVR_BRIDGE_HEADER_DIR}
)

target_link_libraries(drv_alvr
	PRIVATE
		aux_util
		aux_math
		xrt-interfaces
)

# The cdylib produced by `cargo build -p alvr_server_openxr`. Located at
# target/{debug,release}/alvr_server_openxr.{dll,so,dylib}. The Monado build is
# expected to find it via the standard library search path at link / load time.
target_link_libraries(drv_alvr PRIVATE alvr_server_openxr)

# Search hint for the cdylib if it has not been installed system-wide.
if(NOT DEFINED ALVR_SERVER_OPENXR_LIB_DIR)
	set(ALVR_SERVER_OPENXR_LIB_DIR "${CMAKE_SOURCE_DIR}/../target/debug")
endif()
target_link_directories(drv_alvr PRIVATE ${ALVR_SERVER_OPENXR_LIB_DIR})
