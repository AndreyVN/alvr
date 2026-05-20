# Copyright 2026, alvr-org.
# SPDX-License-Identifier: BSL-1.0

if(NOT XRT_FEATURE_COMP_ALVR)
	return()
endif()

add_library(comp_alvr STATIC
	comp_alvr.h
	comp_alvr.c
)

target_include_directories(comp_alvr
	PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}
)

target_link_libraries(comp_alvr
	PRIVATE
		aux_util
		xrt-interfaces
)
