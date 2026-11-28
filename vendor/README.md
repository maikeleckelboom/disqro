# GPUI Base dock patch

`gpui-base-0.10.0` is the published Apache-2.0 package from crates.io. Its pristine import has SHA-256 `4f4e5aff50636223b268a687ad5788ec54fa77c8e57ca9e53d158f19e0425d95` for the compressed crate. The preceding import commit separates upstream source from the local patch.

The dock asks the application skin for the operating size of each visible panel, combines these through its existing tree, and supplies its resizable slots with the resulting ranges. The host can read the center minimum for overflow. The resizer uses measured extents for dragging and preserves preferred proportions through temporary clamping or hidden siblings. Review the differences in `src/dock/dock_area.rs` and `src/resizable/mod.rs` when updating. Remove the Cargo patch and vendored package when the pinned toolkit provides equivalent behavior. The separate Windows presentation patch belongs to Waveform Native.
