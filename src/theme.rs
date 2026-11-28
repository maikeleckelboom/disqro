//! Disqro appearance.
//!
//! A handful of materials describe the whole interface. `install` maps them
//! onto GPUI Kit's theme, so Kit components (Dock, DataTable, menus, inputs)
//! and Disqro's own views draw from the same source. Disqro-specific roles
//! such as the waveform read [`Materials`] directly.
//!
//! A new appearance is a new `Materials` value, not a new frontend: Light and
//! the translucent Glass mode will supply their own materials and, for Glass,
//! a window background effect behind the chrome surfaces.
use gpui_kit::{
    component::{Theme, ThemeMode},
    *,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    #[default]
    Dark,
}

#[derive(Clone, Copy)]
pub struct Materials {
    /// Title bar, navigation and sidebars: the frame around the content.
    pub chrome: Hsla,
    /// Recess behind dock tabs. Selected tabs take their panel surface.
    pub tab_well: Hsla,
    /// The main workspace behind tables and lists.
    pub canvas: Hsla,
    /// Surfaces that sit above the canvas, such as the player.
    pub raised: Hsla,
    /// Menus, popovers and tooltips.
    pub overlay: Hsla,
    /// Structural borders and separators.
    pub line: Hsla,
    /// Borders of controls that accept input.
    pub control_line: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_faint: Hsla,
    /// Used sparingly: selection, focus, active playback.
    pub accent: Hsla,
    pub on_accent: Hsla,
    /// Pointer feedback on rows and quiet buttons.
    pub hover: Hsla,
    /// A selected row or item.
    pub selected: Hsla,
    /// A neutral control fill, such as the play button.
    pub control: Hsla,
    pub control_hover: Hsla,
    pub danger: Hsla,
    pub warning: Hsla,
    pub wave: Wave,
    pub silver: Silver,
}

/// The brushed metal of the play button, the one control with a finish of
/// its own. Everything around it stays flat.
#[derive(Clone, Copy)]
pub struct Silver {
    pub light: Hsla,
    pub dark: Hsla,
    /// The rim around the face.
    pub bezel: Hsla,
    /// The glyph on the face.
    pub ink: Hsla,
    pub grain_light: Hsla,
    pub grain_dark: Hsla,
    /// Highlights and shade on the rim of the face.
    pub edge_light: Hsla,
    pub edge_dark: Hsla,
}

impl Silver {
    fn shade(&self, amount: f32) -> Background {
        let tone = |c: Hsla| Hsla {
            l: (c.l - amount).clamp(0., 1.),
            ..c
        };
        linear_gradient(
            160.,
            linear_color_stop(tone(self.light), 0.),
            linear_color_stop(tone(self.dark), 1.),
        )
    }
    pub fn face(&self) -> Background {
        self.shade(0.)
    }
    pub fn face_hover(&self) -> Background {
        self.shade(-0.04)
    }
    pub fn face_pressed(&self) -> Background {
        self.shade(0.08)
    }
}

/// The waveform keeps its own colors so it never tints the interface.
#[derive(Clone, Copy)]
pub struct Wave {
    pub cursor: Hsla,
    /// The player's timeline lane. It matches the black the waveform
    /// renderer clears to, so the waveform has no visible plate.
    pub lane: Hsla,
}

impl Global for Materials {}

fn hex(v: u32) -> Hsla {
    rgb(v).into()
}
fn hexa(v: u32, alpha: f32) -> Hsla {
    hex(v).opacity(alpha)
}

impl Materials {
    /// Disqro Dark: neutral graphite with a restrained steel-blue accent.
    pub fn dark() -> Self {
        Self {
            chrome: hex(0x141416),
            tab_well: hex(0x101012),
            canvas: hex(0x1b1b1e),
            raised: hex(0x202024),
            overlay: hex(0x242428),
            line: hex(0x2a2a2f),
            control_line: hex(0x34343a),
            // Faint text still labels columns and axes, so it keeps at least
            // 4.5:1 against every surface it sits on.
            text: hex(0xececef),
            text_muted: hex(0xadadb5),
            text_faint: hex(0x8a8a93),
            accent: hex(0x91afcb),
            on_accent: hex(0x0e1217),
            hover: hexa(0xffffff, 0.045),
            selected: hexa(0x91afcb, 0.17),
            control: hexa(0xffffff, 0.08),
            control_hover: hexa(0xffffff, 0.13),
            danger: hex(0xe5675f),
            warning: hex(0xd8ad5c),
            wave: Wave {
                cursor: hex(0xf1f1f3),
                lane: hex(0x000000),
            },
            // Satin rather than mirror: a narrower range of tones than a
            // polished face, so the button reads as metal without glare.
            silver: Silver {
                light: hex(0xdcdde1),
                dark: hex(0xa4a6ad),
                bezel: hex(0x4c4e55),
                ink: hex(0x1b1c20),
                grain_light: hexa(0xffffff, 0.07),
                grain_dark: hexa(0x000000, 0.045),
                edge_light: hexa(0xffffff, 0.55),
                edge_dark: hexa(0x000000, 0.18),
            },
        }
    }
    pub fn for_appearance(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Dark => Self::dark(),
        }
    }
    /// The outline of whatever holds keyboard focus. Rows and tabs reserve a
    /// transparent border of the same width, so focus never moves content.
    pub fn focus_line(&self) -> Hsla {
        self.accent
    }
    /// The resting surface of a text field in the chrome, such as the
    /// address and Library search.
    pub fn field(&self) -> Hsla {
        self.hover
    }
    /// Where a dragged tab or panel will land: a translucent ghost of the
    /// resulting region with a hairline edge, quieter than a selection.
    pub fn preview_fill(&self) -> Hsla {
        self.accent.opacity(0.11)
    }
    pub fn preview_edge(&self) -> Hsla {
        self.accent.opacity(0.55)
    }
}

/// The geometry and type scale every surface shares, in logical pixels.
///
/// These are the Comfortable density: generous enough for long listening
/// sessions, while a dense library still fits. Surfaces take their
/// measurements from here rather than choosing their own, so headers,
/// rows and controls line up across panels that sit side by side.
pub mod size {
    // Type scale.
    /// Rows, tab labels, menus and body text.
    pub const TEXT: f32 = 14.;
    /// Secondary text: artists beside titles, durations, counts and status.
    pub const TEXT_SMALL: f32 = 13.;
    /// Table column headings, section headings, axis and measurement labels.
    pub const LABEL: f32 = 12.;
    /// The recording that is playing, and other single prominent titles.
    pub const TITLE: f32 = 16.;

    // Heights.
    /// The window's title bar.
    pub const TITLE_BAR: f32 = 46.;
    /// Tab strips and panel headers share one height, so their edges line
    /// up across the window.
    pub const HEADER: f32 = 36.;
    /// A content panel's own toolbar, such as the Music Browser's location
    /// and view controls.
    pub const BAR: f32 = 42.;
    /// Table column headings.
    pub const HEADING: f32 = 32.;
    /// Table, tree and list rows.
    pub const ROW: f32 = 32.;
    /// Destinations in the Library sidebar.
    pub const NAV_ROW: f32 = 34.;
    /// A standard control, such as a text field or labeled button.
    pub const CONTROL: f32 = 32.;
    /// An icon button inside a toolbar or header.
    pub const ICON_BUTTON: f32 = 30.;

    // Icons.
    pub const ICON: f32 = 16.;
    pub const ICON_SMALL: f32 = 14.;

    // Spacing.
    /// The horizontal inset of panel content from its edges.
    pub const INSET: f32 = 12.;
    /// Space between groups of related controls.
    pub const GAP: f32 = 8.;
    /// Space between sections of a list, such as Library's.
    pub const SECTION: f32 = 16.;

    // Shape.
    /// Controls, rows and selection.
    pub const RADIUS: f32 = 6.;
    pub const TAB_RADIUS: f32 = 6.;
    /// Minimum usable space for appending a dragged tab to a group.
    pub const TAB_DROP: f32 = 8.;
    /// The inset of a drop preview from the region it describes.
    pub const PREVIEW_INSET: f32 = 4.;
}

/// Figures that line up in columns: durations, counts, levels and times.
pub fn tabular() -> FontFeatures {
    FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)]))
}

pub fn materials(cx: &App) -> &Materials {
    cx.global::<Materials>()
}

/// Single-line text advance in the current interface font, in logical pixels.
pub(crate) fn label_width(label: SharedString, size: Pixels, window: &Window) -> Pixels {
    let run = TextRun {
        len: label.len(),
        font: window.text_style().font(),
        color: window.text_style().color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(label, size, &[run], None)
        .width
        .ceil()
}

/// The interface font. Windows 11 ships Segoe UI Variable; older systems fall
/// back to Segoe UI.
fn interface_font() -> &'static str {
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
    if std::path::Path::new(&windir)
        .join("Fonts")
        .join("SegUIVar.ttf")
        .exists()
    {
        "Segoe UI Variable Text"
    } else {
        "Segoe UI"
    }
}

pub fn install(appearance: Appearance, cx: &mut App) {
    let m = Materials::for_appearance(appearance);
    Theme::change(ThemeMode::Dark, None, cx);
    Theme::update(cx, |t| {
        t.font_family = interface_font().into();
        t.font_size = px(14.);
        t.radius = px(size::RADIUS);
        t.radius_lg = px(8.);
        t.shadow = true;
        t.list.active_highlight = true;

        let c = &mut t.colors;
        c.background = m.canvas;
        c.foreground = m.text;
        c.muted = m.control;
        c.muted_foreground = m.text_muted;
        c.border = m.line;
        c.input = m.control_line;
        c.ring = m.accent.opacity(0.75);
        c.caret = m.text;
        c.selection = m.accent.opacity(0.32);
        c.overlay = hexa(0x000000, 0.35);
        c.window_border = m.line;

        c.accent = m.hover;
        c.accent_foreground = m.text;
        c.primary = m.accent;
        c.primary_hover = hex(0xa5c0d9);
        c.primary_active = hex(0x7f9db9);
        c.primary_foreground = m.on_accent;
        c.secondary = m.control;
        c.secondary_hover = m.control_hover;
        c.secondary_active = hexa(0xffffff, 0.16);
        c.secondary_foreground = m.text;
        c.button = m.control;
        c.button_hover = m.control_hover;
        c.button_active = hexa(0xffffff, 0.16);
        c.button_foreground = m.text;
        c.button_primary = c.primary;
        c.button_primary_hover = c.primary_hover;
        c.button_primary_active = c.primary_active;
        c.button_primary_foreground = c.primary_foreground;
        c.button_secondary = c.secondary;
        c.button_secondary_hover = c.secondary_hover;
        c.button_secondary_active = c.secondary_active;
        c.button_secondary_foreground = c.secondary_foreground;

        c.danger = m.danger;
        c.danger_hover = m.danger.opacity(0.9);
        c.danger_active = m.danger.opacity(0.8);
        c.danger_foreground = m.danger;
        c.button_danger = m.danger;
        c.button_danger_hover = c.danger_hover;
        c.button_danger_active = c.danger_active;
        c.button_danger_foreground = m.on_accent;
        c.warning = m.warning;
        c.warning_foreground = m.warning;

        c.popover = m.overlay;
        c.popover_foreground = m.text;
        c.title_bar = m.chrome;
        c.title_bar_border = m.line;
        c.status_bar = m.chrome;
        c.status_bar_border = m.line;
        c.sidebar = m.chrome;
        c.sidebar_border = m.line;
        c.sidebar_foreground = m.text;
        c.sidebar_accent = m.hover;
        c.sidebar_accent_foreground = m.text;
        c.sidebar_primary = m.accent;
        c.sidebar_primary_foreground = m.on_accent;

        c.tab_bar = m.tab_well;
        c.tab_bar_segmented = m.chrome;
        c.tab = hexa(0x000000, 0.);
        c.tab_active = m.canvas;
        c.tab_foreground = m.text_faint;
        c.tab_active_foreground = m.text;
        c.drag_border = m.accent;
        c.drop_target = m.accent.opacity(0.12);

        c.list = hexa(0x000000, 0.);
        c.list_hover = m.hover;
        c.list_active = m.selected;
        c.list_active_border = hexa(0x000000, 0.);
        c.list_even = hexa(0xffffff, 0.015);
        c.list_head = m.chrome;
        c.list_highlight_foreground = m.accent;

        c.table = m.canvas;
        c.table_head = m.canvas;
        c.table_head_foreground = m.text_faint;
        c.table_row_border = hexa(0x000000, 0.);
        c.table_even = hexa(0xffffff, 0.014);
        c.table_hover = m.hover;
        c.table_active = m.selected;
        c.table_active_border = m.accent.opacity(0.45);

        c.scrollbar = hexa(0x000000, 0.);
        c.scrollbar_thumb = hexa(0xffffff, 0.13);
        c.scrollbar_thumb_hover = hexa(0xffffff, 0.24);
        c.slider_bar = m.accent;
        c.slider_thumb = m.text;
        c.progress_bar = m.accent;
        c.switch = hex(0x3a3a40);
        c.switch_thumb = m.text;
        c.skeleton = m.raised;
        c.link = m.accent;
        c.link_hover = c.primary_hover;
        c.link_active = c.primary_active;
        c.group_box = m.raised;
        c.group_box_foreground = m.text;
    });
    cx.set_global(m);
    // Thin scrollbars that appear on hover, as on Windows 11. GPUI Kit's
    // fade-while-scrolling mode keeps requesting frames for the folder tree,
    // which would cost CPU in an idle window.
    Theme::set_scrollbar_mode(gpui_kit::component::scroll::ScrollbarMode::Hover, cx);
}
