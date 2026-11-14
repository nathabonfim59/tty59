use gpui::{
    App, Background, Hsla, Menu, MenuItem, OsAction, Pixels, Point, Styled, SystemMenuType, Window,
    WindowBackgroundAppearance, linear_color_stop, linear_gradient, point, px, rgb,
};
use gpui_component::scroll::ScrollbarShow;
use gpui_component::{ActiveTheme, Theme, ThemeMode};

use crate::core::actions::*;
use crate::core::config::{Config, WindowBackdrop};
use crate::terminal::view::{
    ClearScrollback, CopyText, CutText, FindInTerminal, FindNext, FindPrevious, PasteText,
    RedoEdit, SelectAll, UndoEdit,
};
use crate::ui::i18n::{L10nKey, t};
use crate::ui::presets;
use crate::ui::presets::Fill;
#[cfg(target_os = "windows")]
use std::sync::OnceLock;

/// Where macOS puts the window's traffic lights over the tab strip: centred on
/// the row of chrome tiles they share the bar with. `x` matches `y`, as AppKit's
/// unified toolbar does (19 and 19 in a 52-point bar).
///
/// `y` is the gap between the window's top edge and the *top* of the button
/// frame, and macOS draws those buttons [`TRAFFIC_LIGHT_BUTTON_H`] tall — so the
/// lights' centre falls half that below it. Deriving the number from
/// [`crate::ui::app::title_strip_centre`], the line every title-bar row puts its
/// contents on, is what keeps the lights on the tile line: the value the 40-point bar needed was
/// rewritten by hand when the bar grew to 48, without the border in it, and left
/// the lights half a point — one device pixel on a Retina panel — below
/// everything beside them.
pub(crate) fn traffic_light_position() -> Point<Pixels> {
    point(px(TRAFFIC_LIGHT_INSET), px(TRAFFIC_LIGHT_INSET))
}

/// The height of AppKit's own standard-window-button frame, which is what the
/// lights are drawn inside. It does not move with the bar's height.
pub(crate) const TRAFFIC_LIGHT_BUTTON_H: f32 = 14.;

// The same content centre drives both the native buttons and the rows beside
// them. Keep the inset const so the leading reserve can follow it at compile
// time instead of maintaining a second copy of the bar's border arithmetic.
const TRAFFIC_LIGHT_INSET: f32 = crate::ui::app::TITLE_STRIP_CENTRE - TRAFFIC_LIGHT_BUTTON_H / 2.;

/// Close to zoom: three 14-point buttons on a 20-point pitch.
const TRAFFIC_LIGHTS_W: f32 = 54.;

/// The room the lights take at the bar's leading edge on macOS: their span,
/// with the inset they sit at on either side.
pub(crate) const TRAFFIC_LIGHTS_LEAD: f32 =
    TRAFFIC_LIGHT_INSET + TRAFFIC_LIGHTS_W + TRAFFIC_LIGHT_INSET;

pub(crate) fn set_menus(cx: &mut App) {
    cx.set_menus([
        Menu::new("tty59").items([
            MenuItem::action(t(L10nKey::AppMenuAbout), About),
            MenuItem::action(t(L10nKey::AppMenuCheckForUpdates), CheckForUpdates),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuSettings), OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu(t(L10nKey::AppMenuServices), SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuHideApp), HideApp),
            MenuItem::action(t(L10nKey::AppMenuHideOthers), HideOthers),
            MenuItem::action(t(L10nKey::AppMenuShowAll), ShowAll),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuQuit), Quit),
        ]),
        Menu::new(t(L10nKey::AppMenuFile)).items([
            MenuItem::action(t(L10nKey::AppMenuNewTab), NewTab),
            MenuItem::action(t(L10nKey::AppMenuNewWorkspace), NewWorkspace),
            MenuItem::action(t(L10nKey::AppMenuNewWorktreeTab), NewWorktreeTab),
            MenuItem::separator(),
            // SSH is one of the reasons to pick tty59 and it had no entry in the
            // menu bar at all — the only routes were ⌘P and Settings, both of
            // which you have to already know about. Same labels as the palette,
            // so there is still one name per thing.
            MenuItem::action(t(L10nKey::CmdSshManageProfiles), OpenSshProfiles),
            MenuItem::action(t(L10nKey::CmdSshReconnect), RestartSshSession),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuSplitRight), SplitRight),
            MenuItem::action(t(L10nKey::AppMenuSplitDown), SplitDown),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuRenameTab), RenameTab),
            MenuItem::action(
                t(L10nKey::AppMenuCopyWorkingDirectory),
                CopyWorkingDirectory,
            ),
            MenuItem::action(t(L10nKey::AppMenuCopySessionId), CopyAgentSessionId),
            MenuItem::action(t(L10nKey::AppMenuForkSession), ForkAgentSession),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuClosePaneTab), CloseActiveTab),
            // Where a Mac user looks for the way to put the window away; it
            // is also the one path that works when another app has taken ⇧⌘W.
            MenuItem::action(t(L10nKey::CmdCloseWindow), CloseWindow),
            MenuItem::action(t(L10nKey::AppMenuCloseOtherTabs), CloseOtherTabs),
            MenuItem::action(t(L10nKey::AppMenuCloseTabsRight), CloseTabsToTheRight),
            MenuItem::action(t(L10nKey::AppMenuReopenClosedTab), ReopenClosedTab),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuRenameWorkspace), RenameWorkspace),
            MenuItem::action(t(L10nKey::AppMenuStopWorkspace), StopWorkspace),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuDeleteWorkspace), DeleteWorkspace),
        ]),
        Menu::new(t(L10nKey::AppMenuEdit)).items([
            MenuItem::os_action(t(L10nKey::AppMenuUndo), UndoEdit, OsAction::Undo),
            MenuItem::os_action(t(L10nKey::AppMenuRedo), RedoEdit, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action(t(L10nKey::AppMenuCut), CutText, OsAction::Cut),
            MenuItem::os_action(t(L10nKey::AppMenuCopy), CopyText, OsAction::Copy),
            MenuItem::os_action(t(L10nKey::AppMenuPaste), PasteText, OsAction::Paste),
            MenuItem::os_action(t(L10nKey::AppMenuSelectAll), SelectAll, OsAction::SelectAll),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuFind), FindInTerminal),
            MenuItem::action(t(L10nKey::AppMenuFindNext), FindNext),
            MenuItem::action(t(L10nKey::AppMenuFindPrevious), FindPrevious),
        ]),
        Menu::new(t(L10nKey::AppMenuView)).items([
            MenuItem::action(t(L10nKey::AppMenuSearchEverywhere), TogglePalette),
            MenuItem::action(t(L10nKey::CmdGoToFile), QuickOpenFile),
            MenuItem::action(t(L10nKey::EditorGoToSymbolAction), EditorGoToSymbol),
            MenuItem::action(t(L10nKey::EditorNavigateBack), EditorNavigateBack),
            MenuItem::action(t(L10nKey::EditorNavigateForward), EditorNavigateForward),
            MenuItem::action(t(L10nKey::EditorSplitRight), EditorSplitRight),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuIncreaseFontSize), IncreaseFontSize),
            MenuItem::action(t(L10nKey::AppMenuDecreaseFontSize), DecreaseFontSize),
            MenuItem::action(t(L10nKey::AppMenuResetFontSize), ResetFontSize),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuLeftSidebar), ToggleLeftPanel),
            MenuItem::action(t(L10nKey::AppMenuRightPanel), ToggleRightPanel),
            MenuItem::action(t(L10nKey::AppMenuCodePanel), ToggleCodePanel),
            MenuItem::action(t(L10nKey::AppMenuTabBarPosition), ToggleTabSidebar),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::CmdSshRemoteFiles), ToggleSftp),
            MenuItem::action(t(L10nKey::CmdSshPortForwarding), ShowSshForwards),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuFocusNextPane), FocusNextPane),
            MenuItem::action(t(L10nKey::AppMenuFocusPreviousPane), FocusPrevPane),
            // No "Zoom Pane" here. It is bound to ⇧⌘↵, and gpui's macOS menu
            // builder has no key equivalent for `enter`: it hands AppKit the
            // literal string "enter", which takes the "e" — so the item drew
            // itself as ⇧⌘E, which is Code Panel's chord, and pressing it
            // opened the code panel. A menu item that teaches the wrong key and
            // points at another command is worse than no menu item. Zoom Pane
            // keeps its place in ⌘P, in a pane's own menu (drawn by us, with
            // the right chord), and on the Keybindings page.
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuClearScrollback), ClearScrollback),
            // No "Enter Full Screen" here: AppKit puts its own at the bottom of
            // any menu named View, so ours sat directly above it — the same
            // command twice, under two different shortcuts. ⌘↵ still works and
            // is listed on the Keybindings page.
        ]),
        Menu::new(t(L10nKey::AppMenuWindow)).items(window_menu_items(cx)),
        Menu::new(t(L10nKey::AppMenuHelp)).items([
            MenuItem::action(t(L10nKey::AppMenuDocumentation), OpenDocumentation),
            MenuItem::action(t(L10nKey::AppMenuKeyboardShortcuts), ShowKeyboardShortcuts),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuJoinDiscord), OpenDiscord),
            MenuItem::action(t(L10nKey::AppMenuReportIssue), ReportIssue),
            MenuItem::separator(),
            MenuItem::action(t(L10nKey::AppMenuRestartServer), RestartDaemon),
        ]),
    ]);
}

fn window_menu_items(cx: &App) -> Vec<MenuItem> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let order = crate::ui::windows::menu_order(cx);
    let store = crate::core::session::WorkspaceStore::all(cx);

    let slot_action = crate::ui::tab_strip::select_workspace_action;

    let mut items = vec![
        MenuItem::action(t(L10nKey::AppMenuMinimize), MinimizeWindow),
        MenuItem::action(t(L10nKey::AppMenuZoom), ZoomWindow),
        MenuItem::separator(),
    ];
    let workspace_start = items.len();
    // Slot order, open and closed interleaved: the item's position is its
    // number and its shortcut, so it cannot move when a window closes (#760).
    // A closed one says so with its age instead.
    for (i, (id, open)) in order.iter().enumerate() {
        let Some(workspace) = store.get(*id) else {
            continue;
        };
        let Some(action) = slot_action(i) else { break };
        let name = crate::ui::machine_mirror::display_name(cx, workspace)
            .unwrap_or_else(|| t(L10nKey::WindowUntitled).to_string());
        let label = if *open {
            name
        } else {
            format!(
                "{}  —  {}",
                name,
                crate::ui::home::relative_time(now, workspace.last_active)
            )
        };
        items.push(MenuItem::Action {
            name: label.into(),
            action,
            os_action: None,
            checked: false,
            disabled: false,
        });
    }
    if items.len() == workspace_start {
        items.push(MenuItem::action(
            t(L10nKey::AppMenuNewWorkspace),
            NewWorkspace,
        ));
    }
    items
}

pub(crate) fn window_background(bg: &presets::ActiveBackground) -> Background {
    window_background_with_alpha(bg, bg.opacity.unwrap_or(1.0))
}

/// The same preset fill with the alpha channel forced to 1 — used by the
/// settings overlay, which must stay opaque (workspace translucency must
/// never show through it) while still rendering the preset's gradient
/// design instead of collapsing to a flat solid color.
pub(crate) fn window_background_opaque(bg: &presets::ActiveBackground) -> Background {
    window_background_with_alpha(bg, 1.0)
}

/// The workspace's own fill: the preset at whatever alpha the window opacity
/// and backdrop material asked for.
pub(crate) fn workspace_background(cx: &App) -> Background {
    match cx.try_global::<presets::ActiveBackground>() {
        Some(bg) => window_background(bg),
        None => cx.theme().background.into(),
    }
}

/// The fill for a full-window overlay (settings, the opened file, the diff
/// view). Always opaque: the overlay covers the whole workspace, so window
/// translucency and the backdrop material must stop at it instead of showing
/// desktop through its text. Overlays pair this with
/// `app::overlay_surface_layers`, because their own opaque fill hides the
/// theme background image the workspace root paints beneath them.
pub(crate) fn overlay_background(cx: &App) -> Background {
    match cx.try_global::<presets::ActiveBackground>() {
        Some(bg) => window_background_opaque(bg),
        None => cx.theme().background.alpha(1.0).into(),
    }
}

fn window_background_with_alpha(bg: &presets::ActiveBackground, alpha: f32) -> Background {
    let stop = |c: u32| -> Hsla {
        let mut h: Hsla = rgb(c).into();
        h.a = alpha;
        h
    };
    match bg.fill {
        Fill::Solid(c) => stop(c).into(),
        Fill::Vertical { top, bottom } => linear_gradient(
            180.,
            linear_color_stop(stop(top), 0.),
            linear_color_stop(stop(bottom), 1.),
        ),
        Fill::Horizontal { left, right } => linear_gradient(
            90.,
            linear_color_stop(stop(left), 0.),
            linear_color_stop(stop(right), 1.),
        ),
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SystemAppearance {
    dark: bool,
}

impl gpui::Global for SystemAppearance {}

fn is_dark(appearance: gpui::WindowAppearance) -> bool {
    matches!(
        appearance,
        gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
    )
}

pub(crate) fn refresh_system_appearance(cx: &mut App) {
    let dark = is_dark(cx.window_appearance());
    cx.set_global(SystemAppearance { dark });
}

pub(crate) fn note_system_appearance(window: &Window, cx: &mut App) {
    let dark = is_dark(window.appearance());
    cx.set_global(SystemAppearance { dark });
}

pub(crate) fn system_dark(cx: &App) -> bool {
    cx.try_global::<SystemAppearance>()
        .is_some_and(|appearance| appearance.dark)
}

pub(crate) fn effective_preset_id(cx: &App) -> String {
    let config = cx.global::<Config>();
    if !config.theme_follow_system {
        config.theme_preset.clone()
    } else if system_dark(cx) {
        config.theme_preset_dark.clone()
    } else {
        config.theme_preset_light.clone()
    }
}

/// Default background alpha used while a backdrop material is active and
/// neither the theme nor the config overrides the opacity — a fully opaque
/// fill would hide the material behind it.
pub(crate) const SYSTEM_MATERIAL_OPACITY: f32 = 0.82;

/// Maps the requested backdrop to the gpui window appearance, falling back
/// down the chain when the OS build predates a material: Mica needs Windows
/// 11 22H2 (build 22621), acrylic needs 1809 (build 17763); below that plain
/// translucency is the best Windows can do. Pure function so the fallback
/// table is unit-testable on every platform.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn windows_background_appearance(
    backdrop: WindowBackdrop,
    blur: bool,
    build: u32,
) -> WindowBackgroundAppearance {
    match backdrop {
        WindowBackdrop::Auto => {
            if blur {
                WindowBackgroundAppearance::Blurred
            } else {
                WindowBackgroundAppearance::Transparent
            }
        }
        WindowBackdrop::Blur => {
            if build >= 17_763 {
                WindowBackgroundAppearance::Blurred
            } else {
                WindowBackgroundAppearance::Transparent
            }
        }
        WindowBackdrop::Mica => {
            if build >= 22_621 {
                WindowBackgroundAppearance::MicaBackdrop
            } else if build >= 17_763 {
                WindowBackgroundAppearance::Blurred
            } else {
                WindowBackgroundAppearance::Transparent
            }
        }
        WindowBackdrop::MicaAlt => {
            if build >= 22_621 {
                WindowBackgroundAppearance::MicaAltBackdrop
            } else if build >= 17_763 {
                WindowBackgroundAppearance::Blurred
            } else {
                WindowBackgroundAppearance::Transparent
            }
        }
        WindowBackdrop::Acrylic => {
            if build >= 22_621 {
                // Native acrylic (DWMSBT_TRANSIENTWINDOW) on Windows 11 22H2+.
                WindowBackgroundAppearance::AcrylicBackdrop
            } else if build >= 17_763 {
                // Classic WCA acrylic (ACCENT_ENABLE_ACRYLICBLURBEHIND).
                WindowBackgroundAppearance::Blurred
            } else {
                WindowBackgroundAppearance::Transparent
            }
        }
        WindowBackdrop::Off => WindowBackgroundAppearance::Transparent,
    }
}

/// The backdrop presets actually worth offering on a given Windows build.
/// Mica, Mica Alt and native acrylic all need Windows 11 22H2 (22621); below
/// that every one of them collapses onto the same classic WCA blur that
/// `Blur` already offers, so listing them would present several choices that
/// render identically — hiding them beats misrepresenting them. Below 1809
/// (17763) there is no blur API at all, so only plain translucency is left.
/// Pure function so the table is unit-testable on every platform.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn supported_backdrops_for(build: u32) -> &'static [WindowBackdrop] {
    if build >= 22_621 {
        &[
            WindowBackdrop::Auto,
            WindowBackdrop::Blur,
            WindowBackdrop::Mica,
            WindowBackdrop::MicaAlt,
            WindowBackdrop::Acrylic,
            WindowBackdrop::Off,
        ]
    } else if build >= 17_763 {
        &[
            WindowBackdrop::Auto,
            WindowBackdrop::Blur,
            WindowBackdrop::Off,
        ]
    } else {
        &[WindowBackdrop::Auto, WindowBackdrop::Off]
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn supported_backdrops() -> &'static [WindowBackdrop] {
    supported_backdrops_for(windows_build_number())
}

#[cfg(target_os = "windows")]
pub(crate) fn windows_build_number() -> u32 {
    static BUILD: OnceLock<u32> = OnceLock::new();
    *BUILD.get_or_init(|| {
        use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
        use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;
        let mut info: OSVERSIONINFOW = unsafe { std::mem::zeroed() };
        // RtlGetVersion is not subject to the app-compat version lying that
        // GetVersionEx is, and unlike GetVersionEx it does not require
        // dwOSVersionInfoSize to be pre-filled; set it anyway so the call
        // matches the documented contract.
        info.dwOSVersionInfoSize = std::mem::size_of_val(&info) as u32;
        let status = unsafe { RtlGetVersion(&mut info) };
        if status == 0 {
            info.dwBuildNumber
        } else {
            // Unknown build: treat it as too old, so the fallback chain picks
            // the most conservative material instead of a silent no-op.
            0
        }
    })
}

#[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
pub(crate) fn resolved_background_appearance(
    backdrop: WindowBackdrop,
    blur: bool,
) -> WindowBackgroundAppearance {
    #[cfg(target_os = "windows")]
    {
        windows_background_appearance(backdrop, blur, windows_build_number())
    }
    #[cfg(not(target_os = "windows"))]
    {
        // `window_backdrop` is a Windows-only setting and this platform has
        // no UI to change or clear it (only "Reset window overrides", which
        // also clobbers opacity). If any variant pinned the appearance
        // here, the local blur switch would silently show a checked state
        // that isn't rendered — so every variant, including Off and Blur,
        // defers to the legacy toggle. (A Windows-synced "off" therefore
        // does not force transparency on macOS/Linux; the local blur switch
        // stays authoritative.)
        if blur {
            WindowBackgroundAppearance::Blurred
        } else {
            WindowBackgroundAppearance::Transparent
        }
    }
}

/// Whether the resolved backdrop on this platform is actually a material —
/// i.e. the window content sits over a blur/mica layer instead of plain
/// transparency. Derived from `resolved_background_appearance` so the
/// opacity defaulting and the appearance resolution can never disagree: on
/// very old builds `Blur`/`Acrylic` fall back to `Transparent`, and a
/// see-through window with no material behind it gets no special opacity.
///
/// `Auto` never counts, even when the legacy blur toggle resolves it to
/// `Blurred`. It is the default for every config written before this
/// setting existed, and those windows were opaque with opaque sidebars —
/// treating them as materials would silently turn an untouched install
/// see-through on first launch after an update. Only an explicit pick in
/// the backdrop dropdown opts a window into the translucent defaults.
pub(crate) fn material_active(backdrop: WindowBackdrop, blur: bool) -> bool {
    cfg!(target_os = "windows")
        && backdrop != WindowBackdrop::Auto
        && matches!(
            resolved_background_appearance(backdrop, blur),
            WindowBackgroundAppearance::Blurred
                | WindowBackgroundAppearance::MicaBackdrop
                | WindowBackgroundAppearance::MicaAltBackdrop
                | WindowBackgroundAppearance::AcrylicBackdrop
        )
}

/// The window's background alpha when neither the config nor the theme sets
/// an explicit opacity: translucent enough to show an active material,
/// fully opaque otherwise. Single source of truth for `apply_theme` and the
/// settings slider, so the two can never drift apart.
pub(crate) fn default_window_opacity(backdrop: WindowBackdrop, blur: bool) -> f32 {
    if material_active(backdrop, blur) {
        SYSTEM_MATERIAL_OPACITY
    } else {
        1.0
    }
}

/// The fill for the workspace's large translucent surfaces (file sidebar,
/// right panel, SFTP panel). While a material is active and the window is
/// translucent, the surface paints on top of the already-alpha window
/// background, so its own alpha stacks (src-over) and the material would
/// show through far less than behind the terminal; a constant 0.15 keeps
/// the backdrop ratio at ~85% of the terminal's at every opacity setting.
/// `theme.sidebar` itself stays opaque — the settings theme picker paints
/// with it on top of the opaque settings overlay and must stay legible.
pub(crate) fn workspace_surface_color(cx: &App) -> Hsla {
    translucent_surface(cx.theme().sidebar, cx)
}

/// The left tab rail's fill: the one tinted surface in the workspace (see
/// `Neutrals::rail`), with the same translucency rule as
/// [`workspace_surface_color`].
pub(crate) fn rail_surface_color(cx: &App) -> Hsla {
    let base: Hsla = rgb(cx.global::<presets::Surfaces>().rail.base).into();
    translucent_surface(base, cx)
}

fn translucent_surface(base: Hsla, cx: &App) -> Hsla {
    let translucent = cx
        .try_global::<presets::ActiveBackground>()
        .and_then(|bg| bg.opacity)
        .is_some_and(|o| o < 1.0);
    if !translucent {
        return base;
    }
    let config = cx.global::<Config>();
    let theme = presets::by_id(cx, &effective_preset_id(cx));
    let blur = config.window_blur.unwrap_or(theme.blur);
    if material_active(config.window_backdrop, blur) {
        base.alpha(0.15)
    } else {
        base
    }
}

/// The backdrop presets offered in the settings dropdown: everything the
/// current build supports, plus the stored value even if it is not
/// supported here (e.g. Mica synced from a newer machine) — so the label
/// always matches what the window actually resolves to instead of quietly
/// showing "Auto" while a fallback appearance is applied.
#[cfg(target_os = "windows")]
pub(crate) fn backdrop_options(current: WindowBackdrop) -> Vec<WindowBackdrop> {
    let mut list = supported_backdrops().to_vec();
    if !list.contains(&current) {
        list.push(current);
    }
    list
}

/// The window's background alpha: the config override, else the theme's,
/// else the backdrop's default.
fn effective_window_opacity(config: &Config, theme: &presets::Theme, blur: bool) -> f32 {
    config
        .window_opacity
        .or(theme.opacity)
        .unwrap_or_else(|| default_window_opacity(config.window_backdrop, blur))
}

/// What a window is actually handed: the resolved backdrop, except that on
/// Windows a fully opaque fill over no DWM material is reported as
/// `Opaque`.
///
/// gpui only rasterizes ClearType (subpixel) glyphs into an `Opaque` window
/// — subpixel coverage cannot be alpha-blended onto whatever is behind a
/// translucent one — so asking for `Transparent` (or the WCA blur) while
/// painting every pixel at alpha 1 silently drops every glyph to grayscale
/// antialiasing for nothing: nothing behind the window can show through
/// anyway (#1120). The DWM materials (Mica / Mica Alt / Acrylic) are left as
/// they are: switching away from one through `Opaque` would not clear the
/// system backdrop type, and they default to a translucent fill regardless.
pub(crate) fn windows_window_appearance(
    resolved: WindowBackgroundAppearance,
    opacity: f32,
) -> WindowBackgroundAppearance {
    match resolved {
        WindowBackgroundAppearance::Transparent | WindowBackgroundAppearance::Blurred
            if opacity >= 1.0 =>
        {
            WindowBackgroundAppearance::Opaque
        }
        other => other,
    }
}

fn window_appearance(
    backdrop: WindowBackdrop,
    blur: bool,
    opacity: f32,
) -> WindowBackgroundAppearance {
    let resolved = resolved_background_appearance(backdrop, blur);
    if cfg!(target_os = "windows") {
        windows_window_appearance(resolved, opacity)
    } else {
        resolved
    }
}

pub(crate) fn background_appearance(cx: &App) -> WindowBackgroundAppearance {
    let config = cx.global::<Config>();
    let theme = presets::by_id(cx, &effective_preset_id(cx));
    let blur = config.window_blur.unwrap_or(theme.blur);
    let opacity = effective_window_opacity(config, &theme, blur);
    window_appearance(config.window_backdrop, blur, opacity)
}

/// The appearance last handed to each live window, so `apply_theme` can skip
/// a `set_background_appearance` that would change nothing.
///
/// Worth the bookkeeping because the call is no longer cheap: with a DWM
/// material selected, gpui answers it by re-setting
/// `DWMWA_SYSTEMBACKDROP_TYPE` and forcing a non-client frame recalculation
/// (`SetWindowPos` with `SWP_FRAMECHANGED`). `apply_theme` runs on *every*
/// `Config` mutation in *every* window, so dragging the opacity slider —
/// which never changes the appearance — would otherwise recalc the frame
/// once per mouse-move sample and visibly stutter the drag.
#[derive(Default)]
struct AppliedAppearance(std::collections::HashMap<gpui::WindowId, WindowBackgroundAppearance>);

impl gpui::Global for AppliedAppearance {}

/// Records `appearance` for `window` and reports whether it differs from
/// what that window was last given. Entries for closed windows are dropped
/// first: window ids come from a slotmap and are reused, so a stale entry
/// could otherwise suppress the very first call for a brand-new window.
fn take_appearance_change(
    window: &Window,
    appearance: WindowBackgroundAppearance,
    cx: &mut App,
) -> bool {
    let id = window.window_handle().window_id();
    let live: std::collections::HashSet<gpui::WindowId> =
        cx.windows().iter().map(|w| w.window_id()).collect();
    let applied = cx.default_global::<AppliedAppearance>();
    applied.0.retain(|known, _| live.contains(known));
    applied.0.insert(id, appearance) != Some(appearance)
}

/// The interface face gpui-component started out with, read once before
/// anything overrode it.
///
/// [`Theme::change`] only rewrites `font_family` when the theme config names
/// one, and none of ours does — so a face written into the theme below stays
/// written for the life of the process. Keeping the stock value here is what
/// lets **Interface font family → Default** put the system face back on the
/// spot; without it the setting would save `None`, leave the window looking
/// exactly as it did, and only come true at the next launch.
struct StockUiFont(gpui::SharedString);

impl gpui::Global for StockUiFont {}

pub(crate) fn apply_theme(mut window: Option<&mut Window>, cx: &mut App) {
    let follow = cx.global::<Config>().theme_follow_system;
    if follow {
        sync_native_appearance(None);
        #[cfg(target_os = "macos")]
        refresh_system_appearance(cx);
    }
    let theme = presets::by_id(cx, &effective_preset_id(cx));
    // Cache the mode beside the machine tree: the daemon is a separate process
    // and reads it back when it spawns a Windows pane, where ConPTY drops an
    // OSC 11 background query before tty59's emulator can answer it. Derived
    // state, so it deliberately stays out of the user's `config.json`.
    crate::core::machine::note_appearance(theme.dark);
    let config = cx.global::<Config>();
    let mode = if theme.dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    let blur = config.window_blur.unwrap_or(theme.blur);
    // A material needs some translucency to be visible; without an explicit
    // opacity override it defaults to SYSTEM_MATERIAL_OPACITY instead of
    // 1.0. Derived from the *resolved* appearance so old builds where
    // Blur/Acrylic fall back to plain transparency stay opaque by default.
    let fill_opacity = effective_window_opacity(config, &theme, blur);
    let opacity = (fill_opacity < 1.0).then_some(fill_opacity);
    if !follow {
        sync_native_appearance(Some(theme.dark));
    }
    // The chrome derivation is resolved once here: one pole, one set of
    // neutrals and surfaces, and every cached ink below is built from them.
    let resolved = theme.resolve();
    let m = resolved.m.clone();
    let surfaces = resolved.sf.clone();
    let sem = theme.semantics_with(&m);
    let interaction = theme.interactions_with(&m);
    let active = theme.active_palette(config.theme_legible_palette);

    let backdrop = config.window_backdrop;
    let ui_font_family = config.ui_font_family.clone();

    if let Some(window) = window.as_deref_mut() {
        let appearance = window_appearance(backdrop, blur, fill_opacity);
        if take_appearance_change(window, appearance, cx) {
            window.set_background_appearance(appearance);
        }
        // Opacity, blur and backdrop are app-wide settings, but they are
        // changed from whichever window the settings page is in — its own,
        // now. Hand the same appearance to every other window too. The one
        // being updated refuses a nested update, and was handled above.
        for other in cx.windows() {
            let _ = other.update(cx, |_, window, cx| {
                if take_appearance_change(window, appearance, cx) {
                    window.set_background_appearance(appearance);
                }
            });
        }
    }

    Theme::change(mode, window.as_deref_mut(), cx);
    cx.set_global(active);
    cx.set_global(presets::ActiveBackground {
        fill: theme.background.clone(),
        opacity,
        image: theme.image.clone(),
    });
    cx.set_global(surfaces.clone());
    cx.set_global(sem);
    cx.set_global(presets::FillPole(resolved.pole));
    cx.set_global(theme.settings_ink_from(&resolved));
    cx.set_global(theme.chrome_ink_from(&resolved));
    cx.set_global(interaction);
    cx.set_global(presets::ActiveAccent(m.accent));
    // Same treatment as `Surfaces`: derived once here rather than recomputed
    // in `render`, because the graph reads it once per visible row per frame
    // and each entry costs a contrast bisection on three surfaces.
    cx.set_global(presets::ActiveLanes(theme.lanes_with(&m)));

    if !cx.has_global::<StockUiFont>() {
        let stock = Theme::global(cx).font_family.clone();
        cx.set_global(StockUiFont(stock));
    }
    let stock = cx.global::<StockUiFont>().0.clone();

    let t = Theme::global_mut(cx);
    // Assigned in both directions, never only when set: this is the one place
    // that decides the chrome's face, so it has to say "back to stock" as
    // plainly as it says "use Inter".
    t.font_family = match ui_font_family {
        Some(family) if !family.trim().is_empty() => family.into(),
        _ => stock,
    };
    let mut base: Hsla = rgb(m.background).into();
    if let Some(o) = opacity {
        base.a = o;
    }
    t.background = base;
    t.foreground = rgb(m.foreground).into();
    t.border = rgb(m.border).into();
    t.secondary = rgb(m.secondary).into();
    // Ink, not fill. gpui-component resolves both of these from its *stock*
    // foreground, so a button's label and the detail panel's section headings
    // were the only text in the window not written in the preset's own colour.
    t.button_foreground = rgb(m.foreground).into();
    t.secondary_foreground = rgb(m.foreground).into();
    t.muted = rgb(m.muted).into();
    t.muted_foreground = rgb(m.muted_foreground).into();
    // Components read both ThemeColor fields and layered tokens. Keep both
    // routes on our palette instead of leaking the component library defaults.
    t.tokens.foreground = Hsla::from(rgb(m.foreground)).into();
    t.tokens.border = Hsla::from(rgb(m.border)).into();
    t.tokens.muted = Hsla::from(rgb(m.muted)).into();
    t.tokens.muted_foreground = Hsla::from(rgb(m.muted_foreground)).into();
    t.tokens.secondary = Hsla::from(rgb(m.secondary)).into();
    t.tokens.secondary_foreground = Hsla::from(rgb(m.foreground)).into();
    t.button = rgb(m.popover).into();
    t.button_secondary = rgb(m.secondary).into();
    t.button_secondary_foreground = rgb(m.foreground).into();
    t.tokens.button = Hsla::from(rgb(m.popover)).into();
    t.tokens.button_foreground = Hsla::from(rgb(m.foreground)).into();
    t.tokens.button_secondary = Hsla::from(rgb(m.secondary)).into();
    t.tokens.button_secondary_foreground = Hsla::from(rgb(m.foreground)).into();
    t.popover = rgb(m.popover).into();
    t.tokens.popover = Hsla::from(rgb(m.popover)).into();
    t.tokens.popover_foreground = Hsla::from(rgb(m.foreground)).into();
    // The field beside the token, not a duplicate of it: gpui-component reads
    // this one for every tooltip, dropdown menu and date picker it draws. Left
    // unset it keeps the stock near-white, so menus and tooltips ignored the
    // preset and came out brighter than the window they float over.
    t.popover_foreground = rgb(m.foreground).into();

    let accent_fill = rgb(interaction.choice);
    let accent_text: Hsla = rgb(interaction.choice_ink).into();
    t.accent = accent_fill.into();
    t.accent_foreground = accent_text;
    t.tokens.accent = Hsla::from(accent_fill).into();
    t.tokens.accent_foreground = accent_text.into();

    let primary_base: Hsla = rgb(interaction.primary.fill).into();
    let primary_hover: Hsla = rgb(interaction.primary_hover).into();
    let primary_active: Hsla = rgb(interaction.primary_pressed).into();
    let primary_text: Hsla = rgb(interaction.primary.on_fill).into();
    t.primary = primary_base;
    t.primary_hover = primary_hover;
    t.primary_active = primary_active;
    t.primary_foreground = primary_text;
    t.button_primary_foreground = primary_text;
    t.tokens.primary = primary_base.into();
    t.tokens.primary_hover = primary_hover.into();
    t.tokens.primary_active = primary_active.into();
    t.tokens.primary_foreground = primary_text.into();
    t.tokens.button_primary = primary_base.into();
    t.tokens.button_primary_hover = primary_hover.into();
    t.tokens.button_primary_active = primary_active.into();
    t.tokens.button_primary_foreground = primary_text.into();

    let steps = |c: u32| {
        (
            Hsla::from(rgb(c)),
            Hsla::from(rgb(presets::mix(c, m.background, 0.15))),
            Hsla::from(rgb(presets::mix(c, m.foreground, 0.15))),
        )
    };

    let (ink, ink_hover, ink_active) = steps(sem.danger.ink);
    let (fill, fill_hover, fill_active) = steps(sem.danger.fill);
    let on_fill = Hsla::from(rgb(sem.danger.on_fill));
    t.danger = ink;
    t.danger_hover = ink_hover;
    t.danger_active = ink_active;
    t.danger_foreground = on_fill;
    t.tokens.danger = fill.into();
    t.tokens.danger_hover = fill_hover.into();
    t.tokens.danger_active = fill_active.into();
    t.tokens.danger_foreground = on_fill.into();
    t.tokens.button_danger = fill.into();
    t.tokens.button_danger_hover = fill_hover.into();
    t.tokens.button_danger_active = fill_active.into();
    t.tokens.button_danger_foreground = on_fill.into();

    let (ink, ink_hover, ink_active) = steps(sem.warning.ink);
    let (fill, fill_hover, fill_active) = steps(sem.warning.fill);
    let on_fill = Hsla::from(rgb(sem.warning.on_fill));
    t.warning = ink;
    t.warning_hover = ink_hover;
    t.warning_active = ink_active;
    t.warning_foreground = on_fill;
    t.tokens.warning = fill.into();
    t.tokens.warning_hover = fill_hover.into();
    t.tokens.warning_active = fill_active.into();
    t.tokens.warning_foreground = on_fill.into();
    t.tokens.button_warning = fill.into();
    t.tokens.button_warning_hover = fill_hover.into();
    t.tokens.button_warning_active = fill_active.into();
    t.tokens.button_warning_foreground = on_fill.into();

    let (ink, ink_hover, ink_active) = steps(sem.success.ink);
    let (fill, fill_hover, fill_active) = steps(sem.success.fill);
    let on_fill = Hsla::from(rgb(sem.success.on_fill));
    t.success = ink;
    t.success_hover = ink_hover;
    t.success_active = ink_active;
    t.success_foreground = on_fill;
    t.tokens.success = fill.into();
    t.tokens.success_hover = fill_hover.into();
    t.tokens.success_active = fill_active.into();
    t.tokens.success_foreground = on_fill.into();
    t.tokens.button_success = fill.into();
    t.tokens.button_success_hover = fill_hover.into();
    t.tokens.button_success_active = fill_active.into();
    t.tokens.button_success_foreground = on_fill.into();

    let (ink, ink_hover, ink_active) = steps(sem.info.ink);
    let (fill, fill_hover, fill_active) = steps(sem.info.fill);
    let on_fill = Hsla::from(rgb(sem.info.on_fill));
    t.info = ink;
    t.info_hover = ink_hover;
    t.info_active = ink_active;
    t.info_foreground = on_fill;
    t.tokens.info = fill.into();
    t.tokens.info_hover = fill_hover.into();
    t.tokens.info_active = fill_active.into();
    t.tokens.info_foreground = on_fill.into();
    t.tokens.button_info = fill.into();
    t.tokens.button_info_hover = fill_hover.into();
    t.tokens.button_info_active = fill_active.into();
    t.tokens.button_info_foreground = on_fill.into();

    // The command-line highlighter paints commands, flags, paths, strings and
    // operators with these, and the find bar flags a bad regex with `red`.
    // Unset they hold gpui-component's stock ramp, one ramp for every dark
    // preset and one for every light one — so the line you were typing kept a
    // palette the output right above it had already left behind.
    t.red = rgb(theme.ansi_ink_with(&m, 1)).into();
    t.green = rgb(theme.ansi_ink_with(&m, 2)).into();
    t.yellow = rgb(theme.ansi_ink_with(&m, 3)).into();
    t.blue = rgb(theme.ansi_ink_with(&m, 4)).into();
    t.magenta = rgb(theme.ansi_ink_with(&m, 5)).into();
    t.cyan = rgb(theme.ansi_ink_with(&m, 6)).into();

    t.link = rgb(sem.link.ink).into();
    t.link_hover = rgb(presets::mix(sem.link.ink, m.foreground, 0.25)).into();
    t.link_active = rgb(presets::mix(sem.link.ink, m.background, 0.20)).into();
    t.tokens.link = Hsla::from(rgb(sem.link.ink)).into();
    t.tokens.link_hover = Hsla::from(rgb(presets::mix(sem.link.ink, m.foreground, 0.25))).into();
    t.tokens.link_active = Hsla::from(rgb(presets::mix(sem.link.ink, m.background, 0.20))).into();

    let knob = if presets::is_lighter(m.background, m.foreground) {
        m.background
    } else {
        m.foreground
    };
    t.tokens.background = Hsla::from(rgb(m.background)).into();
    t.tokens.switch_thumb = Hsla::from(rgb(knob)).into();
    t.tokens.switch = Hsla::from(rgb(surfaces.window.selected)).into();

    // On-switches, sliders and primary actions share the accent role.
    t.tokens.slider_bar = Hsla::from(rgb(m.accent)).into();
    t.tokens.slider_thumb = Hsla::from(rgb(knob)).into();

    t.caret = rgb(m.caret).into();
    t.selection = rgb(m.selection).into();

    let scrollbar_thumb: Hsla = rgb(presets::mix(m.background, m.foreground, 0.18)).into();
    let scrollbar_thumb_hover: Hsla = rgb(presets::mix(m.background, m.foreground, 0.34)).into();
    t.scrollbar = gpui::transparent_black();
    t.scrollbar_thumb = scrollbar_thumb;
    t.scrollbar_thumb_hover = scrollbar_thumb_hover;
    t.tokens.scrollbar = gpui::transparent_black().into();
    t.tokens.scrollbar_thumb = scrollbar_thumb.into();
    t.tokens.scrollbar_thumb_hover = scrollbar_thumb_hover.into();

    // Not `should_auto_hide_scrollbars()`. That preference answers a question
    // about *legacy* scrollbars — the ones that take a gutter out of the layout
    // — and macOS says "don't hide them" for anyone with a mouse plugged in.
    // Ours are overlay bars painted on top of the content, so honouring it
    // parked an opaque bar over the switcher's tab column for the whole time
    // the panel was open, with nothing to fade it out. Every list in the app
    // gets the same bar, so it fades everywhere or nowhere.
    t.scrollbar_show = ScrollbarShow::Scrolling;

    t.radius = px(8.);

    // Flat controls, floating panels. `Theme::shadow` gates exactly one thing —
    // the `shadow_xs` an inline control (button, input, select trigger,
    // checkbox, radio, slider knob) paints under itself — and never the drop
    // shadow on a menu, tooltip or popover, which every one of those draws
    // unconditionally. Left on, every field and button in the window carried a
    // faint lift that nothing else here has: this chrome separates surfaces with
    // low-contrast fills and hairlines, so a control sitting a millimetre above
    // the panel was the one place claiming depth, and it read as a rendering
    // artefact rather than as a material. Panels that really do float keep
    // their shadow.
    t.shadow = false;

    let sidebar_bg = Hsla::from(rgb(m.sidebar));
    // The current tab is a place, not an action: a neutral step of the rail's
    // own ladder plus a heavier title, never the accent. Blue is left to the
    // things that can be pressed.
    let sidebar_sel = rgb(surfaces.sidebar.selected);
    // `t.sidebar` stays the opaque theme token: the settings theme picker
    // paints with it on top of the (opaque) settings overlay, so diluting
    // it would wash out that panel. The workspace sidebar/right-panel
    // surfaces get their translucent variant at render time instead —
    // see `workspace_surface_color`.
    t.sidebar = sidebar_bg.into();
    t.tokens.sidebar = sidebar_bg.into();
    // The lighter tier: every `sidebar_border` site is a pane meeting another
    // pane, and both already carry their own fill. See `Neutrals`.
    t.sidebar_border = rgb(m.divider).into();
    t.sidebar_foreground = rgb(surfaces.sidebar.text_resting).into();
    t.sidebar_accent = sidebar_sel.into();
    t.tokens.sidebar_accent = Hsla::from(sidebar_sel).into();
    t.sidebar_accent_foreground = rgb(surfaces.sidebar.text_selected).into();

    t.list.active_highlight = true;
    t.list_active = rgb(interaction.choice).into();
    t.list_active_border = rgb(interaction.choice).into();
    t.list_hover = rgb(surfaces.popover.hover).into();

    // gpui-component's `input` is the border role, also used by default
    // buttons. Giving it the field background would erase their outlines.
    t.input = rgb(interaction.input_border).into();
    t.tokens.input = Hsla::from(rgb(interaction.input_border)).into();

    let button_hover: Hsla = rgb(surfaces.window.hover).into();
    let button_active: Hsla = rgb(surfaces.window.selected).into();
    t.tokens.button_hover = button_hover.into();
    t.tokens.button_active = button_active.into();
    t.tokens.secondary_hover = button_hover.into();
    t.tokens.secondary_active = button_active.into();
    t.tokens.button_secondary_hover = button_hover.into();
    t.tokens.button_secondary_active = button_active.into();

    t.ring = rgb(m.accent).into();
    t.tokens.ring = Hsla::from(rgb(m.accent)).into();
    // Every splitter and panel edge lights up with this while you drag it, and
    // the drop target for a dragged-in file tints with it. Unset it is a fixed
    // blue from gpui-component's stock theme — the same blue under all nine
    // presets. It is an interaction highlight, so it belongs with the focus
    // ring and the on-switch.
    t.drag_border = rgb(m.accent).into();

    // The code editor's gutter fill and current-line band do not come from
    // `Theme` at all — gpui-component reads them off the *syntax* theme, which
    // is still its stock one. So the editor painted a #0a0a0a strip down the
    // line-number column and a #171717 band across the cursor's line, on top of
    // a preset background that is nowhere near either. That is the black edge.
    //
    // Cleared to transparent rather than repainted with the preset's own fill:
    // the code panel sits on the window background, which may be a gradient or
    // a wallpaper image, and any flat colour would show as a seam against it.
    // The current line and the invisibles become translucent ink for the same
    // reason — they read correctly on light and dark presets alike.
    //
    // A transparent gutter is only safe because our gpui-component fork clips
    // the editor's scrolling content to the right of the line-number column.
    // Upstream leans on the opaque gutter fill to hide horizontally scrolled
    // text, so on a stock build clearing this key lets the text run straight
    // across the line numbers.
    let ink: Hsla = rgb(m.foreground).into();
    let mut highlight = (*t.highlight_theme).clone();
    highlight.style.editor_background = Some(gpui::transparent_black());
    highlight.style.editor_gutter_background = Some(gpui::transparent_black());
    highlight.style.editor_active_line = Some(ink.opacity(0.06));
    highlight.style.editor_line_number = Some(rgb(m.muted_foreground).into());
    highlight.style.editor_active_line_number = Some(ink);
    highlight.style.editor_invisible = Some(ink.opacity(0.25));
    t.highlight_theme = std::sync::Arc::new(highlight);

    #[cfg(target_os = "macos")]
    if let Some(window) = window.as_deref_mut() {
        window.set_traffic_light_position(traffic_light_position());
    }
}

/// Reads the cached [`presets::Semantics`] global populated on theme change
/// so renderers avoid recomputing contrast floors per frame.
///
/// Like [`settings_ink`] and [`chrome_ink`], this panics if [`apply_theme`] has
/// not run, and has no fallback: a window is only ever built after the theme
/// is applied (`main` applies it before the first window, and
/// `Tty59App::with_session` applies it before it builds a view).
pub(crate) fn semantics(cx: &App) -> presets::Semantics {
    *cx.global::<presets::Semantics>()
}

/// Reads the settings page's cached secondary ink and off-switch track,
/// derived once per theme change in [`apply_theme`]. Same required-global
/// invariant as [`semantics`].
pub(crate) fn settings_ink(cx: &App) -> presets::SettingsInk {
    *cx.global::<presets::SettingsInk>()
}

/// The colour every step-toward-the-ink chrome fill moves toward (see
/// `presets::Theme::resolve`), resolved once per theme change in
/// [`apply_theme`]. Painters of fills that are not in `Surfaces` (the
/// settings washes, a port chip's wash) read it. Same required-global
/// invariant as [`semantics`].
pub(crate) fn fill_pole(cx: &App) -> u32 {
    cx.global::<presets::FillPole>().0
}

/// Reads the switcher, search overlay and pane-grip ink, derived once per
/// theme change in [`apply_theme`]. Same required-global invariant as
/// [`semantics`].
pub(crate) fn chrome_ink(cx: &App) -> presets::ChromeInk {
    *cx.global::<presets::ChromeInk>()
}

/// Resolves a theme-free [`crate::core::cli_agent::AgentStatus`] via cached
/// theme semantics so callers read role colours without core UI dependencies.
pub(crate) fn agent_status_semantic(
    status: crate::core::cli_agent::AgentStatus,
    cx: &App,
) -> Option<presets::Semantic> {
    semantics(cx).for_agent_status(status)
}

/// Shared opaque floating surface. In-window GPUI layers cannot blur the
/// terminal beneath them, so depth comes from the edge and shadow instead.
pub(crate) fn floating_surface<T: Styled>(element: T, cx: &App) -> T {
    let theme = cx.theme();
    element
        .bg(theme.popover)
        .text_color(theme.popover_foreground)
        .border_1()
        .border_color(theme.border.opacity(0.65))
        .rounded(crate::ui::rounding::POPOVER_RADIUS)
        .shadow_xl()
}

/// One device pixel: the v4 hairline, 0.5px on a Retina panel and a whole
/// pixel on a 1x one. A literal `px(0.5)` would be antialiased down to half
/// its ink on a 1x display, under the contrast floor `divider` is held to.
pub(crate) fn hairline(window: &Window) -> Pixels {
    px(1. / window.scale_factor().max(1.))
}

/// Tabular figures, so a column of counts, sizes and positions keeps its
/// digits on one grid while the numbers change under it.
pub(crate) fn tabular_figures() -> gpui::FontFeatures {
    gpui::FontFeatures(std::sync::Arc::new(vec![("tnum".to_string(), 1)]))
}

/// The v4 primary button: an inverted neutral — body ink as the fill, the
/// surface it sits on as the label — rather than the accent, which stays with
/// focus and the things that are on. `surface` is the opaque fill under the
/// button, so the label reads as cut out of it. Disabled, gpui-component
/// drops the fill to 15% of itself, the faint well the rest of v4 uses.
pub(crate) fn inverted_button(
    surface: Hsla,
    cx: &App,
) -> gpui_component::button::ButtonCustomVariant {
    let ink = cx.theme().foreground;
    gpui_component::button::ButtonCustomVariant::new(cx)
        .color(ink)
        .foreground(surface)
        .hover(ink.blend(surface.opacity(0.14)))
        .active(ink.blend(surface.opacity(0.24)))
}

pub(crate) fn apply_cursor_hide_mode(cx: &mut App) {
    let mode = if cx.global::<Config>().mouse_hide_while_typing {
        gpui::CursorHideMode::OnTypingAndAction
    } else {
        gpui::CursorHideMode::Never
    };
    cx.set_cursor_hide_mode(mode);
}

#[cfg(target_os = "macos")]
fn sync_native_appearance(dark: Option<bool>) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication,
    };

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let appearance = dark.and_then(|dark| {
        let name = unsafe {
            if dark {
                NSAppearanceNameDarkAqua
            } else {
                NSAppearanceNameAqua
            }
        };
        NSAppearance::appearanceNamed(name)
    });
    NSApplication::sharedApplication(mtm).setAppearance(appearance.as_deref());
}

#[cfg(not(target_os = "macos"))]
fn sync_native_appearance(_dark: Option<bool>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    /// The lights share a line with the chrome tiles they sit beside. gpui
    /// hangs the top of the button frame `y` below the window's top edge, so
    /// their centre falls half a frame under that, and `title_strip_centre` is
    /// where every title-bar row puts its contents. Sixteen and a half points
    /// today, out of the 48pt bar.
    #[test]
    fn the_traffic_lights_sit_on_the_chrome_rows_centre() {
        let position = traffic_light_position();
        assert_eq!(position.x, position.y);
        assert_eq!(
            position.y + px(TRAFFIC_LIGHT_BUTTON_H / 2.),
            crate::ui::app::title_strip_centre()
        );
    }

    /// The strip, editor header and search band start at the lead, so the
    /// lights have to end, with their inset after them, before it does.
    #[test]
    #[cfg(target_os = "macos")]
    fn the_title_bar_lead_clears_the_traffic_lights() {
        let right = traffic_light_position().x + px(TRAFFIC_LIGHTS_W);
        assert!(right + px(TRAFFIC_LIGHT_INSET) <= px(crate::ui::app::TITLE_BAR_LEAD));
    }

    #[gpui::test]
    fn effective_preset_follows_the_cached_system_appearance(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Config(crate::core::config::CoreConfig {
                theme_follow_system: true,
                theme_preset_light: "light-slot".into(),
                theme_preset_dark: "dark-slot".into(),
                ..Default::default()
            }));

            cx.set_global(SystemAppearance { dark: false });
            assert!(!system_dark(cx));
            assert_eq!(effective_preset_id(cx), "light-slot");

            cx.set_global(SystemAppearance { dark: true });
            assert!(system_dark(cx));
            assert_eq!(effective_preset_id(cx), "dark-slot");

            cx.global_mut::<Config>().theme_follow_system = false;
            assert_eq!(effective_preset_id(cx), Config::default().theme_preset);
        });
    }

    #[test]
    fn an_opaque_windows_fill_keeps_cleartype() {
        use WindowBackgroundAppearance::*;
        // A fully opaque fill shows nothing behind the window, so plain
        // translucency or the WCA blur only cost ClearType (#1120).
        assert_eq!(windows_window_appearance(Transparent, 1.0), Opaque);
        assert_eq!(windows_window_appearance(Blurred, 1.0), Opaque);
        // Anything see-through keeps what it asked for.
        assert_eq!(windows_window_appearance(Transparent, 0.9), Transparent);
        assert_eq!(windows_window_appearance(Blurred, 0.82), Blurred);
        // DWM materials are never routed through Opaque.
        for material in [MicaBackdrop, MicaAltBackdrop, AcrylicBackdrop] {
            assert_eq!(windows_window_appearance(material, 1.0), material);
        }
        // The untouched default — Auto, no blur, no opacity override — is the
        // case the report hit: it must come out opaque.
        let resolved =
            windows_background_appearance(crate::core::config::WindowBackdrop::Auto, false, 22_621);
        let opacity = default_window_opacity(crate::core::config::WindowBackdrop::Auto, false);
        assert_eq!(windows_window_appearance(resolved, opacity), Opaque);
    }

    #[test]
    fn windows_background_appearance_falls_back_by_build() {
        use crate::core::config::WindowBackdrop;
        let a = |b: WindowBackdrop, blur: bool, build: u32| {
            windows_background_appearance(b, blur, build)
        };

        // Auto keeps the legacy blur/translucent choice.
        assert_eq!(
            a(WindowBackdrop::Auto, false, 22_621),
            WindowBackgroundAppearance::Transparent
        );
        assert_eq!(
            a(WindowBackdrop::Auto, true, 17_763),
            WindowBackgroundAppearance::Blurred
        );

        // Blur explicitly requests the classic blurred appearance from 1809
        // on; older builds get plain translucency.
        assert_eq!(
            a(WindowBackdrop::Blur, false, 17_763),
            WindowBackgroundAppearance::Blurred
        );
        assert_eq!(
            a(WindowBackdrop::Blur, true, 17_762),
            WindowBackgroundAppearance::Transparent
        );

        // Mica needs Windows 11 22H2; 1809..22H2 falls back to classic
        // acrylic, and below 1809 there is no blur API to fall back to —
        // asking for one there would leave an unbacked see-through window.
        assert_eq!(
            a(WindowBackdrop::Mica, false, 22_621),
            WindowBackgroundAppearance::MicaBackdrop
        );
        assert_eq!(
            a(WindowBackdrop::Mica, false, 22_620),
            WindowBackgroundAppearance::Blurred
        );
        assert_eq!(
            a(WindowBackdrop::Mica, false, 17_762),
            WindowBackgroundAppearance::Transparent
        );
        assert_eq!(
            a(WindowBackdrop::MicaAlt, false, 26_000),
            WindowBackgroundAppearance::MicaAltBackdrop
        );
        assert_eq!(
            a(WindowBackdrop::MicaAlt, false, 17_763),
            WindowBackgroundAppearance::Blurred
        );
        assert_eq!(
            a(WindowBackdrop::MicaAlt, false, 17_762),
            WindowBackgroundAppearance::Transparent
        );
        // A failed RtlGetVersion reports build 0; every material must degrade
        // to plain translucency there rather than request an unsupported one.
        for backdrop in [
            WindowBackdrop::Blur,
            WindowBackdrop::Mica,
            WindowBackdrop::MicaAlt,
            WindowBackdrop::Acrylic,
        ] {
            assert_eq!(
                a(backdrop, false, 0),
                WindowBackgroundAppearance::Transparent,
                "{backdrop:?} on an unknown build must not request a material"
            );
        }

        // Acrylic uses the native DWMSBT_TRANSIENTWINDOW material on 22H2+,
        // classic WCA acrylic from 1809 on, plain translucency below that.
        assert_eq!(
            a(WindowBackdrop::Acrylic, false, 22_621),
            WindowBackgroundAppearance::AcrylicBackdrop
        );
        assert_eq!(
            a(WindowBackdrop::Acrylic, false, 22_620),
            WindowBackgroundAppearance::Blurred
        );
        assert_eq!(
            a(WindowBackdrop::Acrylic, false, 17_763),
            WindowBackgroundAppearance::Blurred
        );
        assert_eq!(
            a(WindowBackdrop::Acrylic, false, 17_762),
            WindowBackgroundAppearance::Transparent
        );

        // Off never requests a material.
        assert_eq!(
            a(WindowBackdrop::Off, false, 26_000),
            WindowBackgroundAppearance::Transparent
        );
        assert_eq!(
            a(WindowBackdrop::Off, true, 26_000),
            WindowBackgroundAppearance::Transparent
        );
    }

    #[test]
    fn auto_never_counts_as_a_material() {
        use crate::core::config::WindowBackdrop;
        // Every config written before this setting existed carries `Auto`,
        // and many of them carry `window_blur: true` from the old switch.
        // Those windows were opaque, with opaque sidebars — an update must
        // not silently turn them see-through, so `Auto` gets the plain 1.0
        // default no matter which way the legacy toggle points.
        for blur in [false, true] {
            assert!(!material_active(WindowBackdrop::Auto, blur));
            assert_eq!(default_window_opacity(WindowBackdrop::Auto, blur), 1.0);
        }
        // `Off` is an explicit "no material" and must behave the same.
        assert!(!material_active(WindowBackdrop::Off, true));
        assert_eq!(default_window_opacity(WindowBackdrop::Off, true), 1.0);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn an_explicitly_picked_material_lowers_the_default_opacity() {
        use crate::core::config::WindowBackdrop;
        // The counterpart to the check above: opting in through the dropdown
        // is what buys the translucent default, on a build that supports it.
        if windows_build_number() < 22_621 {
            return;
        }
        for backdrop in [
            WindowBackdrop::Blur,
            WindowBackdrop::Mica,
            WindowBackdrop::MicaAlt,
            WindowBackdrop::Acrylic,
        ] {
            assert!(material_active(backdrop, false), "{backdrop:?}");
            assert_eq!(
                default_window_opacity(backdrop, false),
                SYSTEM_MATERIAL_OPACITY,
                "{backdrop:?}"
            );
        }
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn non_windows_backdrop_defers_to_the_local_blur_toggle() {
        use crate::core::config::WindowBackdrop;
        // On non-Windows, `window_backdrop` is a Windows-only setting with
        // no local UI to clear it; every variant must defer to the local
        // blur toggle so the switch's checked state always matches what is
        // actually rendered.
        for backdrop in [
            WindowBackdrop::Auto,
            WindowBackdrop::Blur,
            WindowBackdrop::Mica,
            WindowBackdrop::MicaAlt,
            WindowBackdrop::Acrylic,
            WindowBackdrop::Off,
        ] {
            assert_eq!(
                resolved_background_appearance(backdrop, true),
                WindowBackgroundAppearance::Blurred,
                "{backdrop:?} with blur on should resolve to Blurred"
            );
            assert_eq!(
                resolved_background_appearance(backdrop, false),
                WindowBackgroundAppearance::Transparent,
                "{backdrop:?} with blur off should resolve to Transparent"
            );
        }
    }

    #[test]
    fn supported_backdrops_follow_the_build() {
        use crate::core::config::WindowBackdrop;
        assert_eq!(
            supported_backdrops_for(22_621),
            &[
                WindowBackdrop::Auto,
                WindowBackdrop::Blur,
                WindowBackdrop::Mica,
                WindowBackdrop::MicaAlt,
                WindowBackdrop::Acrylic,
                WindowBackdrop::Off,
            ]
        );
        // Pre-22H2: Mica, Mica Alt and Acrylic all collapse onto the very
        // same classic WCA blur that `Blur` already offers, so the dropdown
        // hides them rather than listing choices that render identically.
        assert_eq!(
            supported_backdrops_for(22_620),
            &[
                WindowBackdrop::Auto,
                WindowBackdrop::Blur,
                WindowBackdrop::Off,
            ]
        );
        assert_eq!(
            supported_backdrops_for(17_763),
            &[
                WindowBackdrop::Auto,
                WindowBackdrop::Blur,
                WindowBackdrop::Off,
            ]
        );
        // Every offered preset must resolve to a distinct appearance, or the
        // dropdown is promising a difference the window cannot deliver.
        // `Auto` is exempt: it has no fixed appearance of its own, it mirrors
        // whichever way the blur toggle happens to be set.
        for build in [22_621, 22_620, 17_763, 17_762] {
            let offered: Vec<_> = supported_backdrops_for(build)
                .iter()
                .copied()
                .filter(|b| *b != WindowBackdrop::Auto)
                .collect();
            for (i, a) in offered.iter().enumerate() {
                for b in &offered[i + 1..] {
                    assert_ne!(
                        windows_background_appearance(*a, false, build),
                        windows_background_appearance(*b, false, build),
                        "build {build}: {a:?} and {b:?} render identically"
                    );
                }
            }
        }
        // Below 1809 there is no blur API at all; only plain translucency.
        assert_eq!(
            supported_backdrops_for(17_762),
            &[WindowBackdrop::Auto, WindowBackdrop::Off]
        );
    }

    /// The diagram palette must come from the *active* theme, not from
    /// `MermaidTheme::light()`/`dark()`. Overwrite the theme with unmistakable
    /// values: a mapping that ignores `cx.theme()` misses every assertion here.
    #[gpui::test]
    fn mermaid_theme_follows_the_active_light_theme(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            Theme::change(ThemeMode::Light, None, cx);
            {
                let theme = Theme::global_mut(cx);
                theme.foreground = rgb(0x203040).into();
                theme.background = rgb(0xf0f1f2).into();
                theme.muted = rgb(0xe1e2e3).into();
                theme.secondary = rgb(0xd4d5d6).into();
                theme.secondary_hover = rgb(0xc7c8c9).into();
                theme.border = rgb(0xb0b1b2).into();
                theme.muted_foreground = rgb(0x606162).into();
                theme.danger = rgb(0xb00020).into();
                theme.font_family = "Inter".into();
            }

            let theme = cx.theme();
            let mapped = mermaid_theme(cx);
            assert!(!mapped.dark_mode, "light mode must not map as dark");
            // The diagram font is the renderer's fixed metric-matched stack,
            // so the UI's "Inter" must not surface here.
            assert_eq!(
                mapped.font_family,
                tty59_mermaid::MermaidTheme::light().font_family
            );
            assert_eq!(mapped.background, mermaid_rgba(theme.background));
            assert_eq!(mapped.text, mermaid_rgba(theme.foreground));
            assert_eq!(mapped.primary, mermaid_rgba(theme.muted));
            assert_eq!(mapped.primary_text, mermaid_rgba(theme.foreground));
            assert_eq!(mapped.primary_border, mermaid_rgba(theme.border));
            assert_eq!(mapped.secondary, mermaid_rgba(theme.secondary));
            assert_eq!(mapped.tertiary, mermaid_rgba(theme.secondary_hover));
            assert_eq!(mapped.line, mermaid_rgba(theme.muted_foreground));
            assert_eq!(mapped.edge_label_background, mermaid_rgba(theme.background));
            assert_eq!(mapped.cluster_background, mermaid_rgba(theme.muted));
            assert_eq!(mapped.cluster_border, mermaid_rgba(theme.border));
            assert_eq!(mapped.note_background, mermaid_rgba(theme.secondary));
            assert_eq!(mapped.note_border, mermaid_rgba(theme.border));
            assert_eq!(mapped.actor_background, mermaid_rgba(theme.secondary));
            assert_eq!(mapped.actor_border, mermaid_rgba(theme.border));
            assert_eq!(mapped.error, mermaid_rgba(theme.danger));
        });
    }

    /// A dark theme has to flip `dark_mode` and re-derive every colour; the
    /// dark and light mappings must not come out identical.
    #[gpui::test]
    fn mermaid_theme_follows_the_active_dark_theme(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);

            let theme = cx.theme();
            let dark = mermaid_theme(cx);
            assert!(dark.dark_mode, "dark mode must map as dark");
            // Both palettes share the one metric-matched renderer stack.
            assert_eq!(
                dark.font_family,
                tty59_mermaid::MermaidTheme::light().font_family
            );
            assert_eq!(dark.background, mermaid_rgba(theme.background));
            assert_eq!(dark.text, mermaid_rgba(theme.foreground));
            assert_eq!(dark.primary, mermaid_rgba(theme.muted));
            assert_eq!(dark.primary_border, mermaid_rgba(theme.border));
            assert_eq!(dark.error, mermaid_rgba(theme.danger));

            Theme::change(ThemeMode::Light, None, cx);
            let light = mermaid_theme(cx);
            assert!(!light.dark_mode);
            assert_ne!(
                dark.background, light.background,
                "a theme change must produce a different diagram palette"
            );
        });
    }

    /// merman lays a diagram out against a deterministic Arial-like
    /// measurement profile, and the renderer's fixed font stack is
    /// metric-matched to that profile. Whatever font the UI uses, the
    /// preview must hand the renderer that fixed stack: with any other
    /// face the drawn labels' metrics disagree with the layout's boxes and
    /// clip or overlap. The diagram font must therefore stay the renderer's
    /// default and stay unchanged when the UI font changes, while the
    /// palette keeps following the active theme.
    #[gpui::test]
    fn mermaid_theme_keeps_the_renderer_font_when_the_ui_font_changes(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            Theme::change(ThemeMode::Light, None, cx);

            let before = mermaid_theme(cx);
            let renderer_default = tty59_mermaid::MermaidTheme::light().font_family;

            {
                let theme = Theme::global_mut(cx);
                theme.font_family = "JetBrains Mono".into();
            }

            let theme = cx.theme();
            let after = mermaid_theme(cx);
            assert_eq!(
                after.font_family, renderer_default,
                "the diagram font must be the renderer's default stack, not the UI font"
            );
            assert_ne!(
                after.font_family, "JetBrains Mono",
                "the UI font must not leak into the diagram theme"
            );
            assert_eq!(
                after.font_family, before.font_family,
                "changing the UI font must not change the diagram font"
            );

            // Only the font is decoupled; colours still follow the theme.
            assert_eq!(after.background, mermaid_rgba(theme.background));
            assert_eq!(after.text, mermaid_rgba(theme.foreground));
            assert_eq!(after.primary, mermaid_rgba(theme.muted));
            assert_eq!(after.error, mermaid_rgba(theme.danger));
        });
    }

    /// The ANSI ink order the accent palette is derived in.
    const INK_NAMES: [&str; 6] = ["red", "green", "yellow", "blue", "magenta", "cyan"];

    /// Overwrites the theme with explicit packed colours for the accent
    /// tests: foreground, background, muted, border, then the six ANSI inks
    /// red..cyan. Every value is unmistakable, so a mapping that reads any
    /// other source (stock palettes, defaults) misses the assertions.
    fn accent_theme(
        cx: &mut gpui::App,
        mode: ThemeMode,
        foreground: u32,
        background: u32,
        muted: u32,
        border: u32,
        inks: [u32; 6],
    ) {
        Theme::change(mode, None, cx);
        let theme = Theme::global_mut(cx);
        theme.foreground = rgb(foreground).into();
        theme.background = rgb(background).into();
        theme.muted = rgb(muted).into();
        theme.border = rgb(border).into();
        let [red, green, yellow, blue, magenta, cyan] = inks;
        theme.red = rgb(red).into();
        theme.green = rgb(green).into();
        theme.yellow = rgb(yellow).into();
        theme.blue = rgb(blue).into();
        theme.magenta = rgb(magenta).into();
        theme.cyan = rgb(cyan).into();
    }

    /// Packed `0xRRGGBB` → the renderer's opaque colour, the form the
    /// mapper's fills take.
    fn accent_rgba(color: u32) -> tty59_mermaid::Rgba {
        tty59_mermaid::Rgba {
            r: (color >> 16) as u8,
            g: (color >> 8) as u8,
            b: color as u8,
            a: 255,
        }
    }

    /// An accent fill exactly as the mapper builds it: the ink blended into
    /// the background with [`presets::mix`]'s channel arithmetic.
    fn accent_fill(background: u32, ink: u32, amount: f32) -> tty59_mermaid::Rgba {
        accent_rgba(presets::mix(background, ink, amount))
    }

    /// An accent colour back to packed RGB, for contrast checks.
    fn packed(color: tty59_mermaid::Rgba) -> u32 {
        (color.r as u32) << 16 | (color.g as u32) << 8 | color.b as u32
    }

    /// Six accents, one per ANSI ink in red/green/yellow/blue/magenta/cyan
    /// order, each with the ink itself as its border and the theme's
    /// foreground as its text. The fills blend the ink into the background,
    /// and the blend step is chosen per accent: this theme's foreground
    /// *is* its red ink, so red needs a weaker blend (15 %) to stay
    /// readable, its blue ink is dark enough to need the floor step
    /// (10 %), and the other four read at the full 20 %. The margins
    /// around the 4.5 floor at each step were verified when the fixture
    /// colours were picked, so every assertion pins the exact step taken.
    #[gpui::test]
    fn mermaid_accents_use_the_active_ansi_inks_in_order(cx: &mut TestAppContext) {
        const FOREGROUND: u32 = 0xb01a2c;
        const BACKGROUND: u32 = 0xf0f1f2;
        const INKS: [u32; 6] = [0xb01a2c, 0x3fb950, 0xd4a72c, 0x050810, 0xb98ef9, 0x3fbcc0];
        const STEPS: [f32; 6] = [0.15, 0.20, 0.20, 0.10, 0.20, 0.20];

        cx.update(|cx| {
            gpui_component::init(cx);
            accent_theme(
                cx,
                ThemeMode::Light,
                FOREGROUND,
                BACKGROUND,
                0xe1e2e3,
                0xb0b1b2,
                INKS,
            );

            // The fixture is a legal theme whose body text reads fine; only
            // one ink collides with the foreground.
            assert!(
                presets::contrast(FOREGROUND, BACKGROUND) >= 4.5,
                "the fixture's own body text must clear the floor"
            );

            let mapped = mermaid_theme(cx);
            assert_eq!(mapped.accents.len(), 6, "one accent per ANSI ink");
            for (i, (accent, (&ink, &step))) in mapped
                .accents
                .iter()
                .zip(INKS.iter().zip(STEPS.iter()))
                .enumerate()
            {
                let name = INK_NAMES[i];
                assert_eq!(
                    accent.border,
                    mermaid_rgba(rgb(ink).into()),
                    "{name} border is the ink itself"
                );
                assert_eq!(
                    accent.text, mapped.text,
                    "{name} text is the theme's foreground"
                );
                assert_eq!(
                    accent.fill,
                    accent_fill(BACKGROUND, ink, step),
                    "{name} fill is the ink blended at its chosen step {step}"
                );
                assert!(
                    presets::contrast(FOREGROUND, packed(accent.fill)) >= 4.5,
                    "{name} text keeps the floor on its fill"
                );
            }
        });
    }

    /// The shipped default light and dark presets need no mercy: every ANSI
    /// ink reads at the full 20 % blend, keeps the ink as its border in
    /// red..cyan order, and holds the foreground at ≥ 4.5:1 on its fill —
    /// never the neutral fallback, which would be a bug in the shipped
    /// look, not mercy. The accents are asserted against the *active*
    /// theme values (what `apply_theme` writes into the component theme),
    /// not the preset specs, so a mapping that ignored the live theme
    /// fails here too.
    #[gpui::test]
    fn mermaid_accents_hold_the_contrast_floor_for_stock_light_and_dark(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            for (id, mode) in [("light", ThemeMode::Light), ("dark", ThemeMode::Dark)] {
                let preset = presets::by_id(cx, id);
                let neutrals = preset.neutrals();
                let inks: [u32; 6] = std::array::from_fn(|i| preset.ansi_ink(i + 1));
                accent_theme(
                    cx,
                    mode,
                    neutrals.foreground,
                    neutrals.background,
                    neutrals.muted,
                    neutrals.border,
                    inks,
                );

                let mapped = mermaid_theme(cx);
                assert_eq!(mapped.accents.len(), 6, "{id}: one accent per ANSI ink");
                for (i, (accent, &ink)) in mapped.accents.iter().zip(inks.iter()).enumerate() {
                    let name = INK_NAMES[i];
                    assert_eq!(
                        accent.border,
                        mermaid_rgba(rgb(ink).into()),
                        "{id} {name}: border is the active ANSI ink, not a fallback"
                    );
                    assert_eq!(
                        accent.text, mapped.text,
                        "{id} {name}: text is the theme's foreground"
                    );
                    assert_eq!(
                        accent.fill,
                        accent_fill(neutrals.background, ink, 0.20),
                        "{id} {name}: the full 20 % blend"
                    );
                    assert!(
                        presets::contrast(neutrals.foreground, packed(accent.fill)) >= 4.5,
                        "{id} {name}: text keeps the floor on its fill"
                    );
                }
            }
        });
    }

    /// The setting owns the mode: off yields the empty accent vector — the
    /// renderer's neutral look, byte-identical to pre-Phase-2 — while on,
    /// and crucially a context with no `Config` global at all (standalone
    /// theme tests, previews the settings never reached), yields the full
    /// six: colored diagrams are the shipped default, so absence of
    /// configuration must not silently turn them off.
    #[gpui::test]
    fn mermaid_accents_follow_the_editor_mermaid_colors_setting(cx: &mut TestAppContext) {
        const INKS: [u32; 6] = [0xd1242f, 0x1a7f37, 0x9a6700, 0x0969da, 0x8250df, 0x1b7c83];
        cx.update(|cx| {
            gpui_component::init(cx);
            accent_theme(
                cx,
                ThemeMode::Light,
                0x1c1c1e,
                0xfcfcfb,
                0xe1e2e3,
                0xb0b1b2,
                INKS,
            );

            assert_eq!(
                mermaid_theme(cx).accents.len(),
                6,
                "no Config global: the palette defaults on"
            );

            cx.set_global(Config(crate::core::config::CoreConfig {
                editor_mermaid_colors: true,
                ..Default::default()
            }));
            assert_eq!(
                mermaid_theme(cx).accents.len(),
                6,
                "explicit on keeps the six accents"
            );

            cx.global_mut::<Config>().editor_mermaid_colors = false;
            assert!(
                mermaid_theme(cx).accents.is_empty(),
                "off produces the empty neutral palette"
            );
        });
    }

    /// A theme whose foreground sits on the wrong side of its inks — here
    /// the foreground *is* the yellow ink, and against this background even
    /// 10 % of any of the six inks stays under the floor — gets the
    /// documented failure mode: every accent falls back to the neutral
    /// node pair (`primary` fill, `primary_border` border), and the text
    /// stays the theme's foreground. That fallback is itself below the
    /// floor here (≈ 3.8:1 against `muted`), which is the accepted price of
    /// never swapping the text for black or white: an unreadable theme is
    /// fixed in its palette, not in the diagram renderer.
    #[gpui::test]
    fn mermaid_accents_fall_back_to_neutral_when_no_blend_reads(cx: &mut TestAppContext) {
        const FOREGROUND: u32 = 0x9a6700;
        const INKS: [u32; 6] = [0xb00020, 0x1a7f37, FOREGROUND, 0x0969da, 0x8250df, 0x1b7c83];
        cx.update(|cx| {
            gpui_component::init(cx);
            accent_theme(
                cx,
                ThemeMode::Light,
                FOREGROUND,
                0xf0f1f2,
                0xe1e2e3,
                0xb0b1b2,
                INKS,
            );

            let mapped = mermaid_theme(cx);
            assert_eq!(mapped.accents.len(), 6);
            for (i, accent) in mapped.accents.iter().enumerate() {
                let name = INK_NAMES[i];
                assert_eq!(accent.fill, mapped.primary, "{name} takes the neutral fill");
                assert_eq!(
                    accent.border, mapped.primary_border,
                    "{name} takes the neutral border"
                );
                assert_eq!(
                    accent.text, mapped.text,
                    "{name} text stays the foreground, never black or white"
                );
            }
        });
    }

    /// One failed accent must not drag its siblings down: the gray "yellow"
    /// ink here is the theme's own foreground, so no blend step rescues it
    /// and it alone takes the neutral pair — while the five pastel inks
    /// around it keep their full 20 % blends, their own borders and the
    /// shared foreground text. Each accent walks the ladder alone.
    #[gpui::test]
    fn mermaid_accents_fall_back_alone_without_neutralizing_their_siblings(
        cx: &mut TestAppContext,
    ) {
        const FOREGROUND: u32 = 0x6b6b6d;
        const BACKGROUND: u32 = 0xf0f1f2;
        const INKS: [u32; 6] = [0xfff1f0, 0xf0fff2, FOREGROUND, 0xeef4ff, 0xf8f0ff, 0xeffcff];
        cx.update(|cx| {
            gpui_component::init(cx);
            accent_theme(
                cx,
                ThemeMode::Light,
                FOREGROUND,
                BACKGROUND,
                0xe1e2e3,
                0xb0b1b2,
                INKS,
            );

            let mapped = mermaid_theme(cx);
            assert_eq!(mapped.accents.len(), 6);

            let failed = &mapped.accents[2];
            assert_eq!(
                failed.fill, mapped.primary,
                "the foreground-equal ink takes the neutral fill"
            );
            assert_eq!(failed.border, mapped.primary_border);
            assert_eq!(failed.text, mapped.text);

            for (i, (accent, &ink)) in mapped.accents.iter().zip(INKS.iter()).enumerate() {
                if i == 2 {
                    continue;
                }
                let name = INK_NAMES[i];
                assert_eq!(
                    accent.fill,
                    accent_fill(BACKGROUND, ink, 0.20),
                    "{name} keeps its full blend"
                );
                assert_eq!(
                    accent.border,
                    mermaid_rgba(rgb(ink).into()),
                    "{name} keeps its own ink as border"
                );
                assert_eq!(accent.text, mapped.text);
                assert!(
                    presets::contrast(FOREGROUND, packed(accent.fill)) >= 4.5,
                    "{name} text keeps the floor on its fill"
                );
            }
        });
    }

    #[gpui::test]
    fn switching_theme_presets_updates_cached_semantics_global(cx: &mut TestAppContext) {
        use crate::core::cli_agent::AgentStatus;
        crate::core::config::pin_test_config_dir();
        cx.update(|cx| {
            gpui_component::init(cx);
            let light_expected = presets::by_id(cx, "light").semantics();
            let mut prev_working_ink = None;

            for id in ["light", "dracula", "catppuccin_latte", "light"] {
                cx.set_global(Config(crate::core::config::CoreConfig {
                    theme_follow_system: false,
                    theme_preset: id.into(),
                    ..Default::default()
                }));
                apply_theme(None, cx);

                let expected = presets::by_id(cx, id).semantics();
                let sem = semantics(cx);
                assert_eq!(sem.info.ink, expected.info.ink, "{id}: info ink");
                assert_eq!(sem.info.fill, expected.info.fill, "{id}: info fill");
                assert_eq!(
                    sem.info.on_fill, expected.info.on_fill,
                    "{id}: info on_fill"
                );
                assert_eq!(sem.warning.ink, expected.warning.ink, "{id}: warning ink");
                assert_eq!(sem.success.ink, expected.success.ink, "{id}: success ink");

                let working = agent_status_semantic(AgentStatus::Working, cx)
                    .expect("Working maps to info role");
                assert_eq!(working.ink, expected.info.ink);
                assert_eq!(working.fill, expected.info.fill);
                assert_eq!(working.on_fill, expected.info.on_fill);
                assert_eq!(
                    agent_status_semantic(AgentStatus::Waiting, cx).map(|s| (s.ink, s.fill)),
                    Some((expected.warning.ink, expected.warning.fill))
                );
                assert_eq!(
                    agent_status_semantic(AgentStatus::Done, cx).map(|s| (s.ink, s.fill)),
                    Some((expected.success.ink, expected.success.fill))
                );
                assert!(agent_status_semantic(AgentStatus::Idle, cx).is_none());

                if let Some(prev) = prev_working_ink {
                    assert_ne!(
                        working.ink, prev,
                        "{id}: switching theme updates active status ink"
                    );
                }
                prev_working_ink = Some(working.ink);
            }

            assert_eq!(semantics(cx).info.ink, light_expected.info.ink);
        });
    }

    /// The fill pole and the settings and chrome ink are derived by
    /// `apply_theme` and nowhere else: a theme change installs them, and a
    /// later one replaces them.
    /// Covers the built-ins and a spread of generated palettes, including the
    /// weak ones whose rungs and inks the derivation has to correct.
    #[gpui::test]
    fn apply_theme_installs_and_replaces_the_cached_settings_and_chrome_ink(
        cx: &mut TestAppContext,
    ) {
        crate::core::config::pin_test_config_dir();
        cx.update(|cx| {
            gpui_component::init(cx);
            let mut themes = presets::builtins();
            let grid = presets::generated_theme_grid();
            themes.extend(grid.iter().cloned().step_by(29));
            // The crossover palettes, whose chrome fills invert.
            themes.extend(
                grid.into_iter()
                    .filter(|t| t.background_color() == 0x777777)
                    .step_by(11),
            );
            cx.set_global(presets::Themes(themes.clone()));
            for theme in &themes {
                cx.set_global(Config(crate::core::config::CoreConfig {
                    theme_follow_system: false,
                    theme_preset: theme.id.clone(),
                    ..Default::default()
                }));
                apply_theme(None, cx);

                let pole = theme.resolve().pole;
                assert_eq!(fill_pole(cx), pole, "{}: fill pole", theme.id);
                if theme.background_color() == 0x777777 {
                    assert_eq!(pole, 0xffffff, "{}: a crossover palette inverts", theme.id);
                }
                let settings = settings_ink(cx);
                let expected = theme.settings_ink();
                assert_eq!(settings.sub, expected.sub, "{}: sub", theme.id);
                assert_eq!(settings.warn, expected.warn, "{}: warn", theme.id);
                assert_eq!(
                    settings.switch_off, expected.switch_off,
                    "{}: switch_off",
                    theme.id
                );
                let chrome = chrome_ink(cx);
                let expected = theme.chrome_ink();
                for (name, got, want) in [
                    (
                        "selected_meta",
                        chrome.selected_meta,
                        expected.selected_meta,
                    ),
                    ("quiet_name", chrome.quiet_name, expected.quiet_name),
                    ("avatar_ink", chrome.avatar_ink, expected.avatar_ink),
                    ("keycap_fill", chrome.keycap_fill, expected.keycap_fill),
                    ("keycap_ink", chrome.keycap_ink, expected.keycap_ink),
                    ("grip_idle", chrome.grip_idle, expected.grip_idle),
                    ("search_title", chrome.search_title, expected.search_title),
                    ("panel_title", chrome.panel_title, expected.panel_title),
                    (
                        "unpicked_title",
                        chrome.unpicked_title,
                        expected.unpicked_title,
                    ),
                    (
                        "unpicked_meta",
                        chrome.unpicked_meta,
                        expected.unpicked_meta,
                    ),
                    (
                        "selected_caption",
                        chrome.selected_caption,
                        expected.selected_caption,
                    ),
                    (
                        "port_chip_ink",
                        chrome.port_chip_ink,
                        expected.port_chip_ink,
                    ),
                    (
                        "port_chip_hover_ink",
                        chrome.port_chip_hover_ink,
                        expected.port_chip_hover_ink,
                    ),
                ] {
                    assert_eq!(got, want, "{}: {name}", theme.id);
                }
                // The painted rungs are the ones the roles were floored on.
                let surfaces = theme.surfaces();
                let installed = cx.global::<presets::Surfaces>();
                assert_eq!(installed.popover.selected, surfaces.popover.selected);
                assert_eq!(
                    installed.window.text_selected,
                    surfaces.window.text_selected
                );
            }
        });
    }
}

/// The text view's style for rendered Markdown, wherever tty59 shows it.
///
/// Inline `code` gets a faint neutral fill. The text view's own default is the
/// theme's `accent`, which tty59 also paints selection with, so every code span
/// read as selected text.
pub(crate) fn markdown_style(cx: &gpui::App) -> gpui_component::text::TextViewStyle {
    gpui_component::text::TextViewStyle::default()
        .inline_code_background(cx.theme().foreground.opacity(0.08))
}

/// [`markdown_style`] at the editor's zoom `zoom` (1.0 = 100 %).
///
/// The text view takes its sizes from three sources, and the rem one — body
/// text, lists, the paragraph gap — is handled outside the style, by laying
/// the view out against a scoped rem ([`crate::ui::rem_scope::RemScope`]).
/// What the style carries are the other two: headings grow with
/// `heading_base_font_size` (absolute `px(14)` by default, every level a
/// multiple of the base), and code blocks grow through their refinement,
/// which lands after the text view's own absolute `mono_font_size` and so
/// overrides it. At `zoom == 1.0` the style is byte-for-byte
/// [`markdown_style`], so 100 % renders exactly as the unzoomed preview
/// always did.
pub(crate) fn markdown_style_at(cx: &gpui::App, zoom: f32) -> gpui_component::text::TextViewStyle {
    if zoom == 1.0 {
        return markdown_style(cx);
    }
    let mut style = markdown_style(cx);
    style.heading_base_font_size = gpui::px(
        f32::from(gpui_component::text::TextViewStyle::default().heading_base_font_size) * zoom,
    );
    let mut code_block = gpui::StyleRefinement::default();
    code_block.text.font_size = Some(gpui::px(f32::from(cx.theme().mono_font_size) * zoom).into());
    style.code_block(code_block)
}

/// The palette handed to the mermaid renderer for the active theme.
///
/// Diagrams draw on the preview's own background and use the theme's body ink,
/// neutral fills and borders: `primary`/`secondary`/`tertiary` are neutral
/// surfaces and every border comes from the theme's one border colour.
/// Per-shape accent colours live in `accents`, derived from the active ANSI
/// inks by [`mermaid_accents`] and switched by the `editor_mermaid_colors`
/// setting; the base fields stay neutral so the accent-less look is exactly
/// the one diagrams had before accents existed.
/// `error` is the theme's danger ink, which is the readable red rather than
/// the danger *fill* (that fill's foreground is meant for text on top of it).
pub(crate) fn mermaid_theme(cx: &gpui::App) -> tty59_mermaid::MermaidTheme {
    let theme = cx.theme();

    tty59_mermaid::MermaidTheme {
        dark_mode: theme.is_dark(),
        // merman lays diagrams out against a deterministic Arial-like
        // measurement profile; this fixed stack is metric-matched to it.
        // The UI font must not go here: labels would draw in a face whose
        // metrics disagree with the layout, clipping or overlapping their
        // boxes. The frozen contract carries the stack on `light()`.
        font_family: tty59_mermaid::MermaidTheme::light().font_family,
        background: mermaid_rgba(theme.background),
        text: mermaid_rgba(theme.foreground),
        primary: mermaid_rgba(theme.muted),
        primary_text: mermaid_rgba(theme.foreground),
        primary_border: mermaid_rgba(theme.border),
        secondary: mermaid_rgba(theme.secondary),
        tertiary: mermaid_rgba(theme.secondary_hover),
        line: mermaid_rgba(theme.muted_foreground),
        edge_label_background: mermaid_rgba(theme.background),
        cluster_background: mermaid_rgba(theme.muted),
        cluster_border: mermaid_rgba(theme.border),
        note_background: mermaid_rgba(theme.secondary),
        note_border: mermaid_rgba(theme.border),
        actor_background: mermaid_rgba(theme.secondary),
        actor_border: mermaid_rgba(theme.border),
        error: mermaid_rgba(theme.danger),
        accents: if mermaid_colors(cx) {
            mermaid_accents(theme)
        } else {
            Vec::new()
        },
    }
}

/// The contrast every accent's label must keep on its fill: WCAG's text
/// floor, the same 4.5:1 tty59 holds its own text to.
const MERMAID_ACCENT_FLOOR: f32 = 4.5;

/// How much of each ANSI ink is blended into the background for an accent
/// fill, and the steps the blend walks down when the theme's own
/// foreground cannot read on the first one: 20 %, then 15 %, then 10 %.
/// Less ink means more background, and the background is what the
/// foreground was chosen to read on, so each step down walks the fill back
/// toward legibility without ever touching the text colour.
const MERMAID_ACCENT_MIX_STEPS: [f32; 3] = [0.20, 0.15, 0.10];

/// Whether diagrams get per-shape accent colours: the
/// `editor_mermaid_colors` setting. Defaulting to on when no `Config`
/// global is installed (standalone theme tests, previews the settings
/// never reached) is deliberate: colored diagrams are the shipped look,
/// and absence of configuration must not silently turn them off.
fn mermaid_colors(cx: &App) -> bool {
    cx.try_global::<Config>()
        .is_none_or(|config| config.editor_mermaid_colors)
}

/// The per-shape accent palette for the active theme: one accent per ANSI
/// ink, in red/green/yellow/blue/magenta/cyan order.
///
/// Each accent keeps the theme legible rather than re-authoring it: the
/// border is the ink itself, the text is the theme's own foreground —
/// never black or white, whatever the fill — and the fill is the ink
/// blended into the background, stepping down
/// [`MERMAID_ACCENT_MIX_STEPS`] until the foreground clears
/// [`MERMAID_ACCENT_FLOOR`] on it. An ink no step rescues falls back to
/// the neutral node look (`primary` fill, `primary_border` border) with
/// the foreground as its text; each accent walks the ladder alone, so one
/// unlucky ink never recolors its siblings. In a theme whose own
/// foreground is already unreadable that fallback can be unreadable too —
/// the accepted price of never swapping the text colour, because an
/// unreadable theme is fixed in its palette, not in the diagram renderer.
fn mermaid_accents(theme: &Theme) -> Vec<tty59_mermaid::Accent> {
    let foreground = theme.foreground;
    let background = theme.background;
    let text = mermaid_rgba(foreground);
    let neutral_fill = mermaid_rgba(theme.muted);
    let neutral_border = mermaid_rgba(theme.border);

    [
        theme.red,
        theme.green,
        theme.yellow,
        theme.blue,
        theme.magenta,
        theme.cyan,
    ]
    .into_iter()
    .map(|ink| {
        let fill = MERMAID_ACCENT_MIX_STEPS
            .iter()
            .copied()
            .map(|amount| presets::mix(hsla24(background), hsla24(ink), amount))
            .find(|fill| presets::contrast(hsla24(foreground), *fill) >= MERMAID_ACCENT_FLOOR);
        match fill {
            Some(fill) => tty59_mermaid::Accent {
                fill: mermaid_rgb(fill),
                border: mermaid_rgba(ink),
                text,
            },
            None => tty59_mermaid::Accent {
                fill: neutral_fill,
                border: neutral_border,
                text,
            },
        }
    })
    .collect()
}

/// `Hsla` → packed `0xRRGGBB`, the form [`presets::mix`] and
/// [`presets::contrast`] work in. Alpha is dropped: mixes build opaque
/// fills and contrast is defined on RGB alone.
fn hsla24(color: Hsla) -> u32 {
    let rgb = crate::terminal::palette::hsla_to_rgb(color);
    (rgb.r as u32) << 16 | (rgb.g as u32) << 8 | rgb.b as u32
}

/// Packed `0xRRGGBB` → the renderer's opaque straight-alpha colour.
fn mermaid_rgb(color: u32) -> tty59_mermaid::Rgba {
    tty59_mermaid::Rgba {
        r: (color >> 16) as u8,
        g: (color >> 8) as u8,
        b: color as u8,
        a: 255,
    }
}

/// gpui's float colours (0..=1) → the renderer's straight 8-bit sRGB.
fn mermaid_rgba(color: Hsla) -> tty59_mermaid::Rgba {
    let color = gpui::Rgba::from(color);
    tty59_mermaid::Rgba {
        r: to_byte(color.r),
        g: to_byte(color.g),
        b: to_byte(color.b),
        a: to_byte(color.a),
    }
}

fn to_byte(channel: f32) -> u8 {
    (channel.clamp(0.0, 1.0) * 255.0).round() as u8
}
