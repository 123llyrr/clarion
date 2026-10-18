# HDR10 and RGB color management

Clarion's native Vulkan renderer accepts independently described SDR, PQ and
extended-linear scRGB surfaces. It composes them in a persistent RGBA16F,
extended-sRGB working space and converts the completed scene to the output's
encoding. Ordinary desktop UI remains at the configured SDR reference white;
HDR highlights retain additional range. SDR outputs tone-map HDR clients.

This implementation uses the supplied KWin analysis as architectural evidence,
not as a specification to copy byte for byte. Protocol behavior follows the
locked Wayland XML, and DRM properties follow the kernel's atomic KMS API.

## Configuration

Use the native Vulkan backend and an HDR10-capable display:

```kdl
renderer "vulkan"

output "eDP-1" {
    hdr
    sdr-white-level 203
    // Normally omit this: the default comes from the display's EDID.
    // hdr-peak-luminance 600
    // Optional; an explicitly configured value below 10 disables HDR.
    // max-bpc 10
}
```

`sdr-white-level` accepts 80–500 cd/m². `hdr-peak-luminance` accepts 80–10000
cd/m² and overrides the EDID peak. White is limited to the selected peak.
Missing or invalid EDID luminance uses conservative defaults; it does not bypass
the EDID PQ/BT.2020 capability checks. The peak and frame-average fields remain
distinct when constructing HDR metadata.

The existing runtime command changes the mode without persisting the config:

```sh
clarion msg output eDP-1 hdr on
clarion msg output eDP-1 hdr off
clarion msg outputs
clarion msg --json outputs
```

### HDR display brightness

The bar's control center uses the compositor's per-output HDR brightness when
HDR is active. Hardware backlight commands alone can change their reported value
without changing a panel's PQ luminance. SDR mode retains hardware backlight
control; an HDR adjustment does not overwrite that SDR hardware setting.

```sh
clarion msg output eDP-1 hdr-brightness 50
```

The percentage is finite and bounded to 1–100, with 100 as the default. Like
other `msg output` actions it is transient; `hdr-brightness 50` inside the
output's configuration block persists a preferred starting value. The
`hdr_brightness` field in `msg --json outputs` reports the configured percentage.

Vulkan multiplies absolute linear luminance after tone mapping/clipping and
before the existing final PQ encoding. It dims SDR desktop white and HDR
highlights together, retaining their relative range. It does not apply a gamma
ramp, change EDID peak/reference-white metadata, add a rendering pass, or dim
offscreen scene captures. Brightness changes invalidate framebuffer damage
history without reallocating the swapchain or restaging KMS HDR metadata.

The control center resolves its own output and rechecks HDR state before each
write. It does not change an arbitrary monitor if output selection is ambiguous
or disconnected. A running compositor without the new HDR brightness field
cannot provide this control; update both compositor and shell and log into the
new session. Physical panel luminance still needs on-device verification.

The settings application's display page reports support, committed state,
transition/rejection reason, peak and SDR white. The full output editor exposes
reference white, peak luminance and HDR brightness. Use its existing display preview and confirmation
workflow when editing a physical display configuration.

## Client and renderer contract

- `wp_color_manager_v1` v3 (including v1 clients): immutable image descriptions,
  parametric descriptions, named sRGB/BT.2020/Display P3, sRGB/gamma 2.2/PQ/linear
  transfer functions, perceptual/relative/absolute intents, Windows scRGB and
  BT.2100 descriptions, output information and preferred feedback.
- Color state is double buffered with the surface commit, including synchronized
  subsurfaces. A metadata-only change invalidates the imported texture and
  damages the complete surface. Renderer color publication follows the actual
  buffer replacement after preserving the old resize image, so parent/child
  snapshots retain their original encoding even when metadata changes with size.
- PQ is absolute ST 2084 from 0 to 10000 cd/m². Windows scRGB code value 1 is
  80 cd/m², independently of its 203 cd/m² reference white. Relative transfer
  scale, reference white and mastering/content peak are separate values.
  The Windows scRGB constructor has no content-peak metadata; tone mapping
  currently assumes a 1000 cd/m² content peak. This is a mapping assumption,
  not an encoding limit: linear scRGB code 125 still decodes to 10000 cd/m².
- Sampling unpremultiplies alpha before decoding; RGB is transformed in nits and
  premultiplied again after encoding. Negative and greater-than-one working
  values survive the float intermediate. Final output clips to its volume.
  RGBA16F has finite precision: cancellation between large, signed wide-gamut
  components can introduce small near-black errors. GPU tests check these in
  absolute nits as well as checking ordinary channels against output code values.
- Perceptual conversion uses black/reference adaptation and a maxRGB modified
  Reinhard shoulder, scaling all linear RGB channels together. Shoulder
  measurements use the actual output gamut, so a saturated BT.2020 primary is
  not mistaken for an overbright sRGB component. This is not KWin's ICtCp curve.
- Absolute intent preserves nits through working-space normalization. Relative
  intent adapts reference white without applying perceptual tone mapping.
- Scene, blur and effect caches include color/precision identity. Output scene
  history follows the lifetime of the real framebuffer rather than discarding
  still-live swapchain history. Partial damage converts each changed region
  once, and captures are mapped to ordinary SDR instead of exporting PQ bytes
  as an sRGB PNG.

The global and float SHM formats are advertised only when the initialized
renderer supports the required RGBA16F sampling/blending/transfer features and
push-constant size on every connected output. GLES sessions retain their
existing SDR behavior. Once descriptions have been accepted, a native GPU
failure preserves the last correct frame during recovery; it does not reinterpret
PQ clients through an SDR GLES shader. A newly connected output requiring GLES
is rejected with a status reason in that session.

Format, shader and FP16-scene preparation can defer a cold frame. Headless,
nested and KMS adapters preserve the typed retry result through Smithay's
transparent rendering-error wrappers, retain the previous valid image and
retry after preparation instead of treating it as a device failure.

## Physical output state

HDR requires EDID PQ, CTA static metadata type 1 and BT.2020 RGB; named KMS
metadata/colorspace properties; atomic KMS; a real 10-bit scanout format; and
sufficient connector bpc. Requested, prepared and successfully committed states
are separate. `hdr_active` reports the successfully committed state.

Connector HDR metadata, colorspace, bpc and identity CRTC color transforms are
tested and committed with the matching framebuffer. Older queued/in-flight
frames drain before changing the target; retained frames carry a configuration
epoch. Failed HDR enable keeps the previous valid mode. Failed HDR disable keeps
the old HDR image until an SDR frame and SDR signaling can be committed together.
Retries are bounded and back off. Metadata creation references survive their
staged/active use. DPMS, resume, hot removal and CRTC release reset or restore the
matching signal rather than independently leaving an SDR image under PQ.
Seat handoff performs the inactive color reset with CRTC off before libseat
acknowledges device disable; an acknowledgement guard also covers cleanup errors
and unwinding. A failed kernel cleanup is reported as unknown signal state.

Hardware direct scanout and overlay planes are disabled in this managed path
until per-plane color offload is implemented. Hardware cursors are allowed only
after an SDR signal has been confirmed and the cursor uses default sRGB colors.
HDR, pending/unknown output signaling and color-managed PQ/scRGB cursor surfaces
use the compositor path. Legacy gamma/night-light
ramps cannot be applied to PQ code values: HDR uses identity gamma/degamma/CTM
and rejects legacy gamma changes while active or transitioning.

Deferred target preparation preserves swapchain allocations and invalidates only
damage history. Resetting ages does not release an acquired front or pending slot;
this lets each FP16 scene warm against its actual output buffer instead of
repeatedly replacing it. The final conversion skips color mathematics for exact
default-sRGB SDR output while retaining the FP16 scene and premultiplied clipping.

## Deliberate limits and verification

This is RGB HDR10/scRGB support. ICC profiles, arbitrary chromaticities/white
points, HLG, the color-representation protocol, HDR capture/recording metadata,
hardware color offload and an HDR-aware night-light transform are not advertised
as implemented. YUV uses the renderer's documented limited-range policy; a
tagged BT.2020 source selects the BT.2020 sampler model. It is not a general YUV
range/chroma-location management API.

The CPU reference, actual Wayland wire tests, real GPU numerical tests and KMS
state regressions are separate checks. Render-node GPU readback does not prove
physical HDR presentation, panel calibration or perceived brightness. Physical
KMS activation, VT/DPMS/resume and multi-output validation require a session
running the newly built compositor; do not label nested screenshots as that
acceptance. See the task's validation report for executed checks and remaining
hardware work.

Primary references: [Wayland color-management protocol source](https://gitlab.freedesktop.org/wayland/wayland-protocols/-/blob/main/staging/color-management/color-management-v1.xml),
[kernel KMS color properties](https://docs.kernel.org/gpu/drm-kms.html#color-management-properties).
