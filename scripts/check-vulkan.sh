#!/usr/bin/env bash
# Required Vulkan + EGL/GBM and Wayland sync validation, without DRM display master.
# Optional YUV capabilities are reported explicitly, never counted as exercised.
set -euo pipefail
repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
log_dir=${CLARION_VULKAN_LOG_DIR:-${REALAND_VULKAN_LOG_DIR:-"$repo_dir/target/vulkan-check"}}
mkdir -p -- "$log_dir"

namespace_args=(
    --include-ignored --nocapture --test-threads=1
    --skip render_helpers::vulkan::hardware_tests::lifecycle::gpu_lifecycle_child_fixture
)
case ${CLARION_VULKAN_TEST_SCOPE:-all} in
    all) ;;
    rgb-hdr)
        # These experimental CPU-dumb NV12 interoperability probes are separate
        # from RGB/HDR composition and GPU-produced NV12 sampling. Keep the
        # default all scope strict so their failures cannot be hidden.
        namespace_args+=(
            --skip render_helpers::vulkan::hardware_tests::yuv_cold::native_nv12_cold_import_and_first_submit_owner_work
            --skip render_helpers::vulkan::hardware_tests::yuv_cold::native_nv12_padded_backing_import_and_compatibility_pixels
            --skip render_helpers::vulkan::hardware_tests::yuv_cold::native_nv12_cold_target_preparation_defers_without_owner_wait_or_overwrite
        )
        echo 'RGB/HDR scope: excluding three experimental CPU-dumb NV12 probes.'
        ;;
    *) echo 'CLARION_VULKAN_TEST_SCOPE must be all or rgb-hdr.' >&2; exit 1 ;;
esac

check_validation_log() {
    if grep -Eq 'Validation (Error|Warning)|VUID-|SYNC-HAZARD|Failed to find layer' "$1"; then
        echo "Vulkan validation failed: $1" >&2
        exit 1
    fi
}

command -v vulkaninfo >/dev/null || { echo 'vulkaninfo is required (vulkan-tools).' >&2; exit 1; }
vulkaninfo --summary > "$log_dir/devices.log" 2>&1
check_validation_log "$log_dir/devices.log"
if ! grep -q 'VK_LAYER_KHRONOS_validation' "$log_dir/devices.log"; then
    echo 'Khronos validation layer is required; install it or set VK_LAYER_PATH and LD_LIBRARY_PATH.' >&2
    exit 1
fi
export VK_INSTANCE_LAYERS=VK_LAYER_KHRONOS_validation
export VK_LAYER_VALIDATE_SYNC=1
export CLARION_REQUIRE_VULKAN=1
if (( $# == 0 )); then
    shopt -s nullglob
    set -- /dev/dri/renderD*
fi
(( $# > 0 )) || { echo 'No DRM render nodes found.' >&2; exit 1; }
for node in "$@"; do
    [[ -r "$node" && -w "$node" ]] || { echo "Cannot access $node" >&2; exit 1; }
    export CLARION_VULKAN_TEST_NODE="$node"
    log_file="$log_dir/$(basename -- "$node").log"
    skip_file="$log_dir/$(basename -- "$node").unsupported.log"
    : > "$skip_file"
    cargo test --manifest-path "$repo_dir/Cargo.toml" -p clarion --lib --offline --locked \
        render_helpers::vulkan -- "${namespace_args[@]}" \
        2>&1 | tee "$log_file"
    check_validation_log "$log_file"
    if ! grep -Eq '^test result: ok\. [1-9][0-9]* passed; 0 failed; 0 ignored;' "$log_file"; then
        echo "Vulkan namespace tests did not complete: $log_file" >&2
        exit 1
    fi
    if grep -q 'SKIP:' "$log_file"; then
        # These tests probe optional driver capabilities. A missing capability is
        # distinct from a missing GPU, unavailable Vulkan/EGL or a failed test.
        grep 'SKIP:' "$log_file" > "$skip_file"
        if grep -Ev '^SKIP: (linear NV12 producer requires the selected Intel GPU.s primary DRM node|distinct-plane NV12 producer requires the selected Intel GPU.s primary DRM node|Vulkan driver does not expose sampled linear NV12 DMA-BUF|Intel modifier does not expose disjoint NV12 memory planes|driver cannot import two distinct NV12 DMA-BUF plane FDs)$' "$skip_file"; then
            echo "Vulkan/EGL validation incomplete on $node: a required GPU case was not exercised." >&2
            exit 1
        fi
        echo "Optional YUV coverage unavailable on $node (not exercised):" >&2
        cat -- "$skip_file" >&2
    fi
    # The namespace also contains CPU/cache tests. Its test total must not be
    # presented as hardware coverage. Require an actual selected-device marker.
    selected_device=$(sed -n 's/.*HARDWARE: //p' "$log_file" | awk -v suffix=" ($node)" '
        !found && length($0) > length(suffix) &&
            substr($0, length($0) - length(suffix) + 1) == suffix {
            print substr($0, 1, length($0) - length(suffix)); found = 1
        }')
    [[ -n "$selected_device" ]] || { echo "No hardware test identified $node." >&2; exit 1; }

    # Headless initialization uses the loader's device selection. On multi-GPU
    # machines select the corresponding ICD with VK_DRIVER_FILES; a successful
    # headless test on a different GPU is not acceptance for this render node.
    integration_cases=(
        'headless:backend::headless::tests::native_vulkan_renders_headless_output'
        'client-sync:tests::explicit_sync::wayland_client_acquire_and_release_follow_gpu_use'
        'idle-client-release:tests::explicit_sync::idle_gpu_completion_releases_replaced_and_detached_wayland_buffers'
        'color-client:tests::color_management_native::native_fp16_wayland_metadata_reaches_scene_and_sdr_capture'
        'color-resize:tests::color_management_native::native_resize_snapshot_keeps_previous_parent_and_synchronized_child_colors'
        'color-offscreen:render_helpers::offscreen::native_color_cache_tests::native_offscreen_color_cache_rebuilds_for_sdr_hdr_and_headroom_changes'
        'color-effect:render_helpers::effect_buffer::tests::native_effect_color_cache_rebuilds_for_output_color_and_precision_changes'
        'color-framebuffer:render_helpers::framebuffer_effect::recovery_tests::native_framebuffer_effect_color_cache_rebuilds_for_scene_encoding_changes'
    )
    for integration_case in "${integration_cases[@]}"; do
        integration_name=${integration_case%%:*}
        test_name=${integration_case#*:}
        integration_log="$log_dir/$(basename -- "$node").$integration_name.log"
        cargo test --manifest-path "$repo_dir/Cargo.toml" -p clarion --lib --offline --locked \
            "$test_name" -- --ignored --exact --nocapture --test-threads=1 2>&1 | tee "$integration_log"
        check_validation_log "$integration_log"
        if ! grep -Eq '^test result: ok\. 1 passed; 0 failed; 0 ignored;' "$integration_log" \
            || grep -q 'SKIP:' "$integration_log"; then
            echo "Required Vulkan integration did not execute: $integration_log" >&2
            exit 1
        fi
        if ! grep -Fq "HARDWARE: $selected_device (" "$integration_log"; then
            echo "Integration did not identify $selected_device: $integration_log" >&2
            echo 'Select the matching loader ICD with VK_DRIVER_FILES for this render node.' >&2
            exit 1
        fi
    done
done
echo "Required Vulkan/EGL and eight headless/Wayland/color integrations passed per device. Logs: $log_dir"
echo 'Namespace totals include CPU/cache cases; optional unsupported YUV cases were not exercised.'
