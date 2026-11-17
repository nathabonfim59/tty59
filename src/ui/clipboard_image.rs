//! Pictures on the system clipboard.
//!
//! Two jobs live here because both are about getting a picture off tty59's
//! own memory: staging one in a file (what a terminal paste of a clipboard
//! image does, and what the export below feeds to a tool), and, on Linux,
//! handing it to the desktop's clipboard.
//!
//! gpui's `write_to_clipboard` carries a picture to other applications on
//! macOS and Windows, but on Linux both X11 and Wayland offer text only: an
//! image written through it pastes back into tty59 itself and nowhere else.
//! So on Linux the same PNG is also given to `wl-copy` (Wayland) or `xclip`
//! (X11), the tools that serve a selection for as long as it is owned.

use std::path::PathBuf;

use gpui::RenderImage;

/// Stages a clipboard image as a file under the temp directory, named after
/// its content, and returns where it is. A format an agent cannot be handed
/// (BMP, TIFF…) is written as PNG.
pub(crate) fn write_clipboard_image(img: &gpui::Image) -> Option<PathBuf> {
    use gpui::ImageFormat;
    let dir = std::env::temp_dir().join("tty59-clipboard");
    std::fs::create_dir_all(&dir).ok()?;
    let (ext, transcoded) = match img.format {
        ImageFormat::Png => ("png", None),
        ImageFormat::Jpeg => ("jpg", None),
        ImageFormat::Gif => ("gif", None),
        ImageFormat::Webp => ("webp", None),
        other => ("png", Some(transcode_to_png(&img.bytes, other)?)),
    };
    let data: &[u8] = transcoded.as_deref().unwrap_or(&img.bytes);
    let path = dir.join(format!("paste-{:016x}.{ext}", img.id));
    std::fs::write(&path, data).ok()?;
    Some(path)
}

fn transcode_to_png(bytes: &[u8], format: gpui::ImageFormat) -> Option<Vec<u8>> {
    use gpui::ImageFormat as G;
    let src = match format {
        G::Png => image::ImageFormat::Png,
        G::Jpeg => image::ImageFormat::Jpeg,
        G::Webp => image::ImageFormat::WebP,
        G::Gif => image::ImageFormat::Gif,
        G::Bmp => image::ImageFormat::Bmp,
        G::Tiff => image::ImageFormat::Tiff,
        G::Ico => image::ImageFormat::Ico,
        G::Pnm => image::ImageFormat::Pnm,
        G::Svg => return None,
    };
    let decoded = image::load_from_memory_with_format(bytes, src).ok()?;
    let mut out = Vec::new();
    decoded
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

/// The first frame of a decoded picture as a PNG. The viewer holds frames as
/// straight-alpha BGRA (the order gpui draws in), so the red and blue bytes
/// swap on the way out; nothing is read or written besides the buffer.
pub(crate) fn png_of_first_frame(image: &RenderImage) -> Option<Vec<u8>> {
    let size = image.size(0);
    let width = u32::try_from(i32::from(size.width)).ok()?;
    let height = u32::try_from(i32::from(size.height)).ok()?;
    let mut rgba = image.as_bytes(0)?.to_vec();
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(width, height, rgba)?;
    let mut out = Vec::new();
    buffer
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

/// A tool that puts a PNG on the Linux clipboard.
#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tool {
    WlCopy,
    Xclip,
}

/// What to run for a [`Tool`]: the program, its arguments, and the file its
/// standard input reads, for the tool that takes no file argument.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ExportCommand {
    pub(crate) program: &'static str,
    pub(crate) args: Vec<std::ffi::OsString>,
    pub(crate) stdin: Option<PathBuf>,
}

#[cfg(any(target_os = "linux", test))]
impl Tool {
    /// The command that puts `file`, a PNG, on the clipboard.
    pub(crate) fn command(self, file: &std::path::Path) -> ExportCommand {
        match self {
            // wl-copy reads what it offers from stdin and has no file argument.
            Tool::WlCopy => ExportCommand {
                program: "wl-copy",
                args: vec!["--type".into(), "image/png".into()],
                stdin: Some(file.to_path_buf()),
            },
            Tool::Xclip => ExportCommand {
                program: "xclip",
                args: vec![
                    "-selection".into(),
                    "clipboard".into(),
                    "-t".into(),
                    "image/png".into(),
                    "-i".into(),
                    file.as_os_str().to_owned(),
                ],
                stdin: None,
            },
        }
    }
}

/// Which tool serves the clipboard here: `wl-copy` on a Wayland session that
/// has it, else `xclip` on an X11 one that has it, else none. `env` answers
/// for an environment variable and `on_path` for a program on `PATH`; an
/// empty variable is as good as an unset one.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn choose_tool(
    env: impl Fn(&str) -> Option<String>,
    on_path: impl Fn(&str) -> bool,
) -> Option<Tool> {
    let set = |name: &str| env(name).is_some_and(|v| !v.is_empty());
    if set("WAYLAND_DISPLAY") && on_path("wl-copy") {
        Some(Tool::WlCopy)
    } else if set("DISPLAY") && on_path("xclip") {
        Some(Tool::Xclip)
    } else {
        None
    }
}

#[cfg(target_os = "linux")]
pub(crate) use export::ClipboardExport;
#[cfg(all(target_os = "linux", test))]
pub(crate) use export::{ExportBackend, ExportJob};

#[cfg(target_os = "linux")]
mod export {
    use std::io;
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
    use std::time::Duration;

    use gpui::{BackgroundExecutor, Context, Window};

    use super::{PathBuf, choose_tool};
    use crate::ui::i18n::{L10nKey, t};

    /// How long a launched tool is waited for. Both fork their server and the
    /// process we launched exits at once; one that is still there after this is
    /// left to the reaper list instead of being waited on.
    const EXPORT_REAP_LIMIT: Duration = Duration::from_secs(5);
    /// How often a launched tool is asked whether it has exited.
    const EXPORT_POLL: Duration = Duration::from_millis(50);

    /// A way to put a staged file on the clipboard. The system one runs
    /// `wl-copy` or `xclip`; tests install their own, which never spawn
    /// either.
    pub(crate) trait ExportBackend: Send + Sync + 'static {
        /// Starts exporting `file`: `Ok(None)` when there is no tool to do it.
        fn start(&self, file: &Path) -> io::Result<Option<Box<dyn ExportJob>>>;
    }

    /// An export that was started.
    pub(crate) trait ExportJob: Send {
        /// Whether it has finished (or can no longer be asked), without
        /// waiting for it.
        fn finished(&mut self) -> bool;
    }

    #[cfg_attr(test, allow(dead_code))]
    struct System;

    impl ExportBackend for System {
        fn start(&self, file: &Path) -> io::Result<Option<Box<dyn ExportJob>>> {
            let Some(tool) = choose_tool(|name| std::env::var(name).ok(), on_path) else {
                return Ok(None);
            };
            let command = tool.command(file);
            let mut process = Command::new(command.program);
            process
                .args(&command.args)
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            match &command.stdin {
                Some(input) => process.stdin(std::fs::File::open(input)?),
                None => process.stdin(Stdio::null()),
            };
            Ok(Some(Box::new(Spawned(process.spawn()?))))
        }
    }

    /// Spawns nothing and finds no tool: what tests get unless they install one.
    #[cfg(test)]
    struct Inert;

    #[cfg(test)]
    impl ExportBackend for Inert {
        fn start(&self, _file: &Path) -> io::Result<Option<Box<dyn ExportJob>>> {
            Ok(None)
        }
    }

    #[cfg_attr(test, allow(dead_code))]
    struct Spawned(Child);

    impl ExportJob for Spawned {
        fn finished(&mut self) -> bool {
            !matches!(self.0.try_wait(), Ok(None))
        }
    }

    /// Whether an executable called `program` is in a directory of `PATH`.
    #[cfg_attr(test, allow(dead_code))]
    fn on_path(program: &str) -> bool {
        use std::os::unix::fs::PermissionsExt as _;
        let Some(path) = std::env::var_os("PATH") else {
            return false;
        };
        std::env::split_paths(&path).any(|dir| {
            std::fs::metadata(dir.join(program))
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    }

    fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Asks every export on the reaper list whether it is done, and drops the
    /// ones that are: that is what collects the exited child.
    fn reap(reaper: &Mutex<Vec<Box<dyn ExportJob>>>) {
        locked(reaper).retain_mut(|job| !job.finished());
    }

    #[derive(Default)]
    struct Slots {
        in_flight: bool,
        /// The file asked for while one was exporting: only the newest is kept.
        pending: Option<PathBuf>,
    }

    enum Outcome {
        Exported,
        NoTool,
        Failed,
    }

    struct Shared {
        backend: Arc<dyn ExportBackend>,
        slots: Mutex<Slots>,
        /// Exports still running after [`EXPORT_REAP_LIMIT`]: asked again on
        /// the next copy and when this goes.
        reaper: Mutex<Vec<Box<dyn ExportJob>>>,
        /// Whether the "no tool" notice was shown: once per process.
        noticed: Arc<AtomicBool>,
    }

    impl Drop for Shared {
        fn drop(&mut self) {
            reap(&self.reaper);
        }
    }

    impl Shared {
        async fn run(&self, file: &Path, executor: &BackgroundExecutor) -> Outcome {
            reap(&self.reaper);
            let mut job = match self.backend.start(file) {
                Ok(Some(job)) => job,
                Ok(None) => return Outcome::NoTool,
                Err(e) => {
                    log::warn!("exporting {} to the clipboard: {e}", file.display());
                    return Outcome::Failed;
                }
            };
            let mut waited = Duration::ZERO;
            while !job.finished() {
                if waited >= EXPORT_REAP_LIMIT {
                    locked(&self.reaper).push(job);
                    break;
                }
                executor.timer(EXPORT_POLL).await;
                waited += EXPORT_POLL;
            }
            Outcome::Exported
        }
    }

    /// Hands staged pictures to the desktop's clipboard, one at a time.
    ///
    /// Clones share one state: a copy asked for while another is being
    /// exported waits in a single slot (the newest replaces an older one) and
    /// goes when the running one ends, so two tools never run at once.
    #[derive(Clone)]
    pub(crate) struct ClipboardExport {
        shared: Arc<Shared>,
    }

    impl Default for ClipboardExport {
        /// The system's: `wl-copy` or `xclip`, whichever this session has.
        fn default() -> Self {
            static NOTICED: OnceLock<Arc<AtomicBool>> = OnceLock::new();
            let noticed = NOTICED.get_or_init(Arc::default).clone();
            // Tests that copy without installing a stand-in must never start a
            // real tool on a machine that happens to have one.
            #[cfg(test)]
            let backend: Arc<dyn ExportBackend> = Arc::new(Inert);
            #[cfg(not(test))]
            let backend: Arc<dyn ExportBackend> = Arc::new(System);
            Self::with_backend(backend, noticed)
        }
    }

    impl ClipboardExport {
        pub(crate) fn with_backend(
            backend: Arc<dyn ExportBackend>,
            noticed: Arc<AtomicBool>,
        ) -> Self {
            Self {
                shared: Arc::new(Shared {
                    backend,
                    slots: Mutex::default(),
                    reaper: Mutex::default(),
                    noticed,
                }),
            }
        }

        /// Exports `file`, a PNG, to the clipboard without making the caller
        /// wait: the tool is launched from the background executor. With no
        /// tool to do it, says so once.
        pub(crate) fn submit<T: 'static>(
            &self,
            file: PathBuf,
            window: &mut Window,
            cx: &mut Context<T>,
        ) {
            {
                let mut slots = locked(&self.shared.slots);
                if slots.in_flight {
                    slots.pending = Some(file);
                    return;
                }
                slots.in_flight = true;
            }
            let shared = self.shared.clone();
            cx.spawn_in(window, async move |_, cx| {
                let mut file = file;
                loop {
                    let running = shared.clone();
                    let executor = cx.background_executor().clone();
                    let outcome = cx
                        .background_spawn(async move { running.run(&file, &executor).await })
                        .await;
                    if matches!(outcome, Outcome::NoTool)
                        && !shared.noticed.swap(true, Ordering::SeqCst)
                    {
                        let _ = cx.update(|window, cx| {
                            window.push_notification(t(L10nKey::ImageCopyNoTool), cx)
                        });
                    }
                    let mut slots = locked(&shared.slots);
                    match slots.pending.take() {
                        Some(next) => file = next,
                        None => {
                            slots.in_flight = false;
                            return;
                        }
                    }
                }
            })
            .detach();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn env_of(set: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            set.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    }

    fn path_of(have: &'static [&'static str]) -> impl Fn(&str) -> bool {
        move |name| have.contains(&name)
    }

    /// Two pixels in the viewer's order (BGRA, straight alpha): opaque blue and
    /// a half-transparent orange. They come out as RGBA, unchanged.
    #[test]
    fn the_first_frame_becomes_a_png_with_its_colours_and_alpha() {
        let bgra = vec![255, 0, 0, 255, 50, 100, 200, 128];
        let frame = image::Frame::new(image::RgbaImage::from_raw(2, 1, bgra).unwrap());
        let png = png_of_first_frame(&RenderImage::new(vec![frame])).expect("encodes");
        let back = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .unwrap()
            .to_rgba8();
        assert_eq!(back.dimensions(), (2, 1));
        assert_eq!(back.get_pixel(0, 0).0, [0, 0, 255, 255], "blue stays blue");
        assert_eq!(
            back.get_pixel(1, 0).0,
            [200, 100, 50, 128],
            "alpha survives"
        );
    }

    #[test]
    fn a_picture_without_frames_has_no_png() {
        assert!(png_of_first_frame(&RenderImage::new(Vec::<image::Frame>::new())).is_none());
    }

    #[test]
    fn wayland_with_wl_copy_uses_wl_copy() {
        let tool = choose_tool(
            env_of(&[("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")]),
            path_of(&["wl-copy", "xclip"]),
        );
        assert_eq!(tool, Some(Tool::WlCopy));
    }

    #[test]
    fn wayland_without_wl_copy_falls_back_to_xclip_on_x11() {
        let tool = choose_tool(
            env_of(&[("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")]),
            path_of(&["xclip"]),
        );
        assert_eq!(tool, Some(Tool::Xclip));
    }

    #[test]
    fn no_session_or_no_tool_is_no_tool() {
        // A tool without its session is not used, and a session without its tool.
        assert_eq!(
            choose_tool(env_of(&[]), path_of(&["wl-copy", "xclip"])),
            None
        );
        assert_eq!(
            choose_tool(
                env_of(&[("WAYLAND_DISPLAY", "wayland-0")]),
                path_of(&["xclip"])
            ),
            None
        );
        assert_eq!(
            choose_tool(env_of(&[("DISPLAY", ":0")]), path_of(&["wl-copy"])),
            None
        );
        // An empty variable is unset.
        assert_eq!(
            choose_tool(env_of(&[("DISPLAY", "")]), path_of(&["xclip"])),
            None
        );
    }

    #[test]
    fn the_commands_are_exactly_what_each_tool_takes() {
        let file = Path::new("/tmp/tty59-clipboard/paste-1.png");
        assert_eq!(
            Tool::WlCopy.command(file),
            ExportCommand {
                program: "wl-copy",
                args: vec!["--type".into(), "image/png".into()],
                stdin: Some(file.to_path_buf()),
            }
        );
        assert_eq!(
            Tool::Xclip.command(file),
            ExportCommand {
                program: "xclip",
                args: ["-selection", "clipboard", "-t", "image/png", "-i"]
                    .into_iter()
                    .map(Into::into)
                    .chain([file.as_os_str().to_owned()])
                    .collect(),
                stdin: None,
            }
        );
    }
}
