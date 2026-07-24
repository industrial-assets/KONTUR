//! KONTUR palette — the design-system colour tokens as ratatui styles.
//!
//! A monochrome control-room look: bone foreground on warm near-black, emphasis
//! carried by weight/dim/reverse-video rather than hue. Brick red is the ONE
//! identity accent (the КОНТУР dot, the rule under the wordmark) and is spent
//! sparingly. Functional colour is confined to what the console actually
//! decodes: diff add=green / remove=red / hunk=cyan, and the GO/NO-GO verdict.
//!
//! This is a *full branded ground*: the console paints its own bone-on-black
//! surface rather than inheriting the operator's terminal theme, so both seats
//! see the identical control room. Values are the design system's tokens
//! (`tokens/colors.css`), which were themselves sampled from this render.
//!
//! Design rules encoded here (docs/UX-kontur.md §2):
//! - **Emphasis is spent once.** Only [`alarm`] (red ground) and [`reverse`]
//!   (bone ground) are loud; everything else stays calm.
//! - **Square and flat.** Borders are the only structural device — 1px
//!   hairlines in [`LINE`]; no shadow, no gradient, no colour on chrome.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Block;

// — Base neutrals: warm near-black → bone —
/// Deepest ground — the console backdrop (`--k-black`).
pub const GROUND: Color = Color::Rgb(0x0e, 0x0d, 0x0c);
/// Raised / selected surface fill (`--k-panel-2`). "Elevation" is a lighter
/// fill, never a shadow.
pub const RAISED: Color = Color::Rgb(0x1e, 0x1c, 0x15);
/// Brightest bone — BOLD text, headings (`--k-bone-100`).
pub const BONE_100: Color = Color::Rgb(0xec, 0xe7, 0xdb);
/// Default foreground — terminal body text (`--k-bone-200`).
pub const BONE_200: Color = Color::Rgb(0xd6, 0xd0, 0xc4);
/// Dim chatter, machine log (`--k-dim`).
pub const DIM: Color = Color::Rgb(0x8a, 0x85, 0x78);
/// Very dim — disabled, hints, footers (`--k-faint`).
pub const FAINT: Color = Color::Rgb(0x5b, 0x57, 0x4d);
/// Default hairline border / box-drawing rule (`--k-line`).
pub const LINE: Color = Color::Rgb(0x32, 0x2f, 0x28);

// — Brand accent: brick red (the КОНТУР dot + rule) —
/// The one identity accent (`--k-red`). Spent sparingly.
pub const RED: Color = Color::Rgb(0xbf, 0x3b, 0x26);
/// Brighter red — deletions, no-go, failure (`--k-red-bright`).
pub const RED_BRIGHT: Color = Color::Rgb(0xdb, 0x4a, 0x2f);

// — Functional semantics (diff / verdict / status) —
/// Additions · GO (`--k-green`).
pub const GREEN: Color = Color::Rgb(0x7f, 0xa6, 0x5c);
/// `@@` hunk headers (`--k-cyan`).
pub const CYAN: Color = Color::Rgb(0x5b, 0x9a, 0x97);
/// Caution / needs-attention — used, never loud (`--k-amber`).
pub const AMBER: Color = Color::Rgb(0xc8, 0x92, 0x3a);

// — Diff aliases (what diffview decodes) —
/// Diff addition foreground (`+`).
pub const DIFF_ADD: Color = GREEN;
/// Diff deletion foreground (`-`).
pub const DIFF_DEL: Color = RED_BRIGHT;
/// Diff hunk-header foreground (`@@`).
pub const DIFF_HUNK: Color = CYAN;

// ---------------------------------------------------------------------------
// Semantic style helpers — reference these, not the raw ramp.
// ---------------------------------------------------------------------------

/// The whole-frame ground fill: bone body text on warm near-black. Painted
/// once over the frame so unset widgets inherit the branded surface.
pub fn base() -> Style {
    Style::default().fg(BONE_200).bg(GROUND)
}

/// Strong text — headings, section labels, the important line (`--k-bone-100`).
pub fn strong() -> Style {
    Style::default()
        .fg(BONE_100)
        .add_modifier(Modifier::BOLD)
}

/// Dim chatter — the calm default for status the operator isn't acting on.
pub fn dim() -> Style {
    Style::default().fg(DIM)
}

/// Very dim — footers, disabled hints.
pub fn faint() -> Style {
    Style::default().fg(FAINT)
}

/// The brick-red identity accent (wordmark dot, banner КОНТУР, the rule).
pub fn accent() -> Style {
    Style::default().fg(RED).add_modifier(Modifier::BOLD)
}

/// Needs-you status — a fleet row that wants this seat's key (`--state-needs-you`).
pub fn needs_you() -> Style {
    Style::default().fg(RED).add_modifier(Modifier::BOLD)
}

/// Caution — used, never loud (escalation notes). The only place amber appears.
pub fn caution() -> Style {
    Style::default().fg(AMBER).add_modifier(Modifier::BOLD)
}

/// Failure — a non-zero exit, a broken chain, a failed merge.
pub fn failure() -> Style {
    Style::default()
        .fg(RED_BRIGHT)
        .add_modifier(Modifier::BOLD)
}

/// Verified success — a green ✓ (chain verified, merged, additions).
pub fn success() -> Style {
    Style::default().fg(GREEN)
}

/// Reverse-video attention (loud): bone ground, black ink. The louder of the
/// two calm-breaking treatments — used for the single thing that needs a human.
pub fn reverse() -> Style {
    Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
}

/// Alarm (the single loudest treatment): brick-red ground, bone ink. Reserved
/// for a frozen session — never more than one on screen.
pub fn alarm() -> Style {
    Style::default()
        .fg(BONE_100)
        .bg(RED)
        .add_modifier(Modifier::BOLD)
}

// — Verdict-key states (gate verdict bar) —
/// A cast, revealed GO (`--state-go`).
pub fn go() -> Style {
    Style::default().fg(GREEN).add_modifier(Modifier::BOLD)
}
/// A cast, revealed NO-GO (`--state-nogo`).
pub fn nogo() -> Style {
    Style::default()
        .fg(RED_BRIGHT)
        .add_modifier(Modifier::BOLD)
}
/// A key still awaiting its verdict (`--state-await`).
pub fn awaiting() -> Style {
    Style::default().fg(DIM)
}
/// A cast-but-sealed key — neutral bone, never revealing the value
/// (`--state-sealed`). Blind review: the value must not read from its colour.
pub fn sealed() -> Style {
    Style::default().fg(BONE_200)
}

/// A standard KONTUR panel: a hairline-bordered box with an UPPERCASE mono
/// title on the top border (ratatui's `Block::bordered().title(...)`). Square
/// corners, no shadow — the border is the only structural device.
pub fn panel<S: Into<String>>(title: S) -> Block<'static> {
    Block::bordered()
        .border_style(Style::default().fg(LINE))
        .title(Line::styled(title.into(), strong()))
}
