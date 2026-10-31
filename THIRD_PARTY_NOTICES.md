# Third-party notices

Clarion is distributed under the [GNU General Public License, version 3 or later](LICENSE).
Individual files and dependencies retain their copyright notices and license terms.
This index accompanies those records; it does not replace source headers or dependency licenses.

| Component or record | Notice |
| --- | --- |
| Upstream foundation (Niri) | [GPL-3.0-or-later](LICENSE), [Niri repository](https://github.com/YaLTeR/niri) by YaLTeR and contributors |
| Project source provenance | [Source provenance and retained notices](resources/licenses/realand-origin.txt) |
| Smithay and local adaptations | [MIT license](third_party/smithay/LICENSE.txt), [adaptation record](third_party/smithay/README.realand.txt) |
| Blade Graphics 0.7.1 and suboptimal-swapchain fix | [MIT license](third_party/blade-graphics/LICENSE), [patch record](third_party/blade-graphics/CLARION_PATCHES.md) |
| GPUI 0.2.2 and layer-shell adaptations | [Apache-2.0 license](third_party/gpui/LICENSE-APACHE), [patch record](third_party/gpui/CLARION_PATCHES.md); launcher probe input derives from GPUI's input example |
| Window effects and Shell references | [Attribution](resources/licenses/denial-GPL-3.0-or-later.txt), [adaptation notes](docs/denial-reference.txt) |
| Canvas-related adaptations | [MIT notice](resources/licenses/nourish-MIT.txt), [adaptation notes](docs/nourish-port.md) |
| Glass rendering sources | [Source and license record](resources/licenses/liquid-glass-sources.txt), [GPL text](resources/licenses/niri-glass-GPL-3.0.txt) |
| GTK-related interfaces | [LGPL text](resources/licenses/gtk-COPYING.txt), source-file notices |
| Removed Apple-derived assets | [Removal manifest](resources/licenses/removed-apple-assets.sha256), [policy and restore path](clarion-shell/assets/shell/README.txt) |

Other Rust and system-library dependencies are identified by their manifests, lockfiles and retained source notices.

## Apple-derived assets

No Apple artwork or fonts are distributed with Clarion. Icons, the Apple wordmark
and the SF / PingFang fonts extracted during reference work were removed from the
tree on 2026-10-17: Apple's license does not permit redistribution, and
GPL-3.0-or-later cannot grant that right.

`resources/licenses/removed-apple-assets.sha256` records every removed file with
its SHA-256. `scripts/import-shell-assets.sh` restores an identical copy on a
machine whose owner supplies the source files; imported files are git-ignored.
The extraction records in `clarion-shell/assets/shell/provenance.json` and
`clarion-settings/assets/shell/provenance.json` are audit metadata, not a
redistribution grant.
