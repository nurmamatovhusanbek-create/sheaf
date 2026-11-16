# egui-winit 0.37.0, with one change

A copy of the published `egui-winit` 0.37.0 (MIT OR Apache-2.0, https://github.com/emilk/egui) with one
change in `src/lib.rs` (search for `SHEAF PATCH`): when a transparent window is created while
`SHEAF_RENDERER=wgpu`, it is created with `WS_EX_NOREDIRECTIONBITMAP`.

Why: the Direct3D 12 fallback draws through a DirectComposition visual so that the action wheel's corners
can be see-through. egui offers no way to ask winit for a window without a redirection bitmap, and with one
the corners show as an opaque white rectangle (seen on the Windows CI runner, run 22).

It is used through `[patch.crates-io]` in the root `Cargo.toml`. Drop it when egui lets a viewport ask for
this itself.
