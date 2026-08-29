# Keep Web Preview In One Window

Keep Windows WebView previews inside the GPUI application window. Move WebView2
work to a dedicated STA worker that owns a private child-window host inside the
main window, and prevent native floating surfaces from competing with that child
window.

The result must retain one application top-level window across resize and close
transitions. Reject approaches that create a companion window or blank the
preview whenever an overlay appears.
