# Launcher macOS accessibility patch

Source: the unmodified `accesskit_macos` 0.27.0 crate, upstream commit
`42e53b0d829e7a0b34dc8803bb06012db2e80cc6`, `adapters/macos` in
<https://github.com/AccessKit/accesskit>. The manifest is the crates.io manifest.
The upstream licenses are included here. No other dependencies are vendored.

Local changes in `src/node.rs`:

- Map `Role::Heading` to `AXHeading`, matching the native heading role already
  used for `Role::DocSubtitle`.
- Map `Role::DisclosureTriangle` to `NSAccessibilityDisclosureTriangleRole` and
  expose its expanded state as its boolean value. Existing value-change
  notifications then announce toggles through the native adapter.
- Implement `isAccessibilityExpanded` and allow that selector only when the
  AccessKit node supplies an expanded state. egui's `CollapsingHeader` already
  supplies it, including immediately after a click, before its animation ends.
- Add regression tests for these mappings.

The launcher keeps using `Role::Heading` and `Role::DisclosureTriangle` on every
platform; this patch affects only the macOS adapter. Remove this patch override
when a compatible upstream release provides these fixes. Upstream 0.27.2 and
main still contain the original heading mapping and lack expanded-state mapping
as checked on 2026-11-15.
