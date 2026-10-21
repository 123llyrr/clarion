# Branding provenance

The text prompts and JSON manifests here record the origin of Clarion's artwork.
The production images are maintained in:

- `clarion-shell/assets/icons/`: menu-bar marks, application icons, and About artwork.
- `clarion-settings/assets/icons/`: the eight preference-pane icons embedded by Rust.

The original full-size design drafts and the duplicate settings-icon ZIP were
removed from this documentation directory after checking their references and
preserving the production assets. They were also removed from the published Git
history to keep clones small. The maintainer's offline backup retains the
original files and the complete pre-cleanup history, including these paths:

- `docs/branding/clarion-*.png`
- `docs/branding/releases/clarion-huangshan-circle-v*.png`
- `docs/branding/settings-icons-realistic-v1/*.png`
- `docs/branding/settings-icons-realistic-v1.zip`

`settings-icons-realistic-v1/manifest.json` and `prompts.json` describe that
historical generation bundle. Their image filenames refer to the archived
full-size drafts. Build and installation scripts use the production directories
above; they do not load images from these historical manifests.

Local PNG and ZIP drafts are ignored within this directory. Keep future runtime
artwork in the appropriate asset directory with its attribution and license.
