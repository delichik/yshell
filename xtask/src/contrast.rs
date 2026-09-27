//! `cargo xtask contrast` - WCAG contrast checks for the design tokens in
//! `ui/theme.slint`.
//!
//! The checker reads the light and dark values of every `<color>` token,
//! composites translucent tokens (the file stores them as `#RRGGBBAA`, alpha
//! last) over the surface they are drawn on, and verifies the combinations the
//! design language promises to keep accessible (`ui-winui3-design-language.md`
//! §8):
//!
//! * body-size text: >= 4.5:1 (WCAG 2.x AA, normal text)
//! * icons / large text: >= 3:1 (WCAG 2.x AA, non-text contrast)
//!
//! Failures are printed with the token names, the raw and effective colors and
//! the measured ratio, and the command exits non-zero so it can gate CI later.
//! The theme file is never modified: findings are meant to be reported and
//! fixed in a separate change.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _, Result};

use crate::context::Context;

/// Theme file checked when `--theme-file` is not given.
const DEFAULT_THEME_FILE: &str = "ui/theme.slint";

/// Options collected for `cargo xtask contrast`.
pub struct ContrastOptions {
    /// Theme file to read; relative paths are resolved from the repo root.
    pub theme_file: Option<PathBuf>,
    /// Theme(s) to check.
    pub theme: ThemeSelection,
    /// Print passing icon rows as well (body rows are always listed).
    pub verbose: bool,
}

/// Which of the two token sets to check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeSelection {
    Both,
    Light,
    Dark,
}

impl ThemeSelection {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "light" => Ok(Self::Light),
            "dark" => Ok(Self::Dark),
            "both" => Ok(Self::Both),
            other => bail!("unknown theme `{other}` (expected light, dark or both)"),
        }
    }
}

/// `cargo xtask contrast`.
pub fn run(ctx: &Context, options: &ContrastOptions) -> Result<()> {
    let file = match &options.theme_file {
        Some(path) if path.is_absolute() => path.clone(),
        Some(path) => ctx.root.join(path),
        None => ctx.root.join(DEFAULT_THEME_FILE),
    };
    let source = fs::read_to_string(&file)
        .with_context(|| format!("failed to read theme file {}", file.display()))?;
    let tokens = Tokens::parse(&source)
        .with_context(|| format!("failed to parse theme file {}", file.display()))?;
    let checks = checks();

    println!("contrast: checking {}", file.display());
    println!("  WCAG 2.x relative luminance; body text >= 4.50:1, icons / large text >= 3.00:1");

    let themes: &[(bool, &str)] = match options.theme {
        ThemeSelection::Both => &[(false, "light"), (true, "dark")],
        ThemeSelection::Light => &[(false, "light")],
        ThemeSelection::Dark => &[(true, "dark")],
    };

    let mut total = 0usize;
    let mut failures = 0usize;
    for &(dark, label) in themes {
        failures += report_theme(label, &file, &tokens, dark, &checks, options.verbose)?;
        total += checks.len();
    }

    if failures == 0 {
        println!();
        println!("contrast: {total} checks passed");
        Ok(())
    } else {
        println!();
        println!("contrast: {failures} of {total} checks failed (details above)");
        let _ = std::io::stdout().flush();
        bail!("contrast: {failures} of {total} checks are below the required ratio");
    }
}

// ----------------------------------------------------------------------- colors

/// An sRGB color with straight (non-premultiplied) alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rgba {
    r: u8,
    g: u8,
    b: u8,
    a: u8,
}

impl Rgba {
    const TRANSPARENT: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };

    /// Parse a `#RRGGBB` or `#RRGGBBAA` literal (alpha last, as Slint writes it).
    fn parse(text: &str) -> Result<Self> {
        let digits = text.strip_prefix('#').unwrap_or(text);
        let valid = !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_hexdigit());
        if !valid {
            bail!("`{text}` is not a hex color");
        }
        match digits.len() {
            6 => Ok(Self {
                r: parse_hex_pair(digits, 0)?,
                g: parse_hex_pair(digits, 2)?,
                b: parse_hex_pair(digits, 4)?,
                a: 255,
            }),
            8 => Ok(Self {
                r: parse_hex_pair(digits, 0)?,
                g: parse_hex_pair(digits, 2)?,
                b: parse_hex_pair(digits, 4)?,
                a: parse_hex_pair(digits, 6)?,
            }),
            _ => bail!("color `{text}` must be #RRGGBB or #RRGGBBAA (alpha last)"),
        }
    }

    /// Exact source-over composite of `self` on top of `background`.
    fn over(self, background: Self) -> Self {
        let source = f64::from(self.a) / 255.0;
        let backdrop = f64::from(background.a) / 255.0;
        let alpha = source + backdrop * (1.0 - source);
        if alpha == 0.0 {
            return Self::TRANSPARENT;
        }
        let blend = |fg: u8, bg: u8| -> u8 {
            let value =
                (source * f64::from(fg) + backdrop * f64::from(bg) * (1.0 - source)) / alpha;
            value.round().clamp(0.0, 255.0) as u8
        };
        Self {
            r: blend(self.r, background.r),
            g: blend(self.g, background.g),
            b: blend(self.b, background.b),
            a: (alpha * 255.0).round() as u8,
        }
    }

    /// `#RRGGBB` of the effective (composited) color.
    fn rgb_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// `#RRGGBB`, or `#RRGGBBAA` when the token carries alpha.
    fn token_hex(self) -> String {
        if self.a == 255 {
            self.rgb_hex()
        } else {
            format!("#{:02X}{:02X}{:02X}{:02X}", self.r, self.g, self.b, self.a)
        }
    }
}

fn parse_hex_pair(digits: &str, offset: usize) -> Result<u8> {
    u8::from_str_radix(&digits[offset..offset + 2], 16)
        .with_context(|| format!("invalid hex color `#{digits}`"))
}

/// WCAG relative luminance of an opaque color.
fn luminance(color: Rgba) -> f64 {
    fn channel(value: u8) -> f64 {
        let value = f64::from(value) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
}

/// WCAG contrast ratio; symmetric in its arguments.
fn contrast_ratio(first: Rgba, second: Rgba) -> f64 {
    let (a, b) = (luminance(first), luminance(second));
    let (lighter, darker) = if a >= b { (a, b) } else { (b, a) };
    (lighter + 0.05) / (darker + 0.05)
}

// ----------------------------------------------------------------------- tokens

/// Raw `<color>` token expressions exactly as written in the theme file.
struct Tokens {
    raw: BTreeMap<String, String>,
}

impl Tokens {
    /// Extract every `property <color> name: expression;` declaration.
    fn parse(source: &str) -> Result<Self> {
        const MARKER: &str = "property <color>";
        let mut raw = BTreeMap::new();
        for (index, line) in source.lines().enumerate() {
            let line_number = index + 1;
            let line = match line.find("//") {
                Some(comment) => &line[..comment],
                None => line,
            };
            let Some((_, rest)) = line.split_once(MARKER) else {
                continue;
            };
            let Some((name, value)) = rest.split_once(':') else {
                bail!("line {line_number}: `<color>` property without `:`");
            };
            let name = name.trim();
            let value = value.trim().trim_end_matches(';').trim();
            if name.is_empty() || value.is_empty() {
                bail!("line {line_number}: malformed `<color>` property");
            }
            if raw.insert(name.to_string(), value.to_string()).is_some() {
                bail!("line {line_number}: duplicate color token `{name}`");
            }
        }
        if raw.is_empty() {
            bail!("no `<color>` tokens found");
        }
        Ok(Self { raw })
    }

    /// Resolve a token to a concrete color for the given theme.
    fn resolve(&self, name: &str, dark: bool) -> Result<Rgba> {
        self.eval_name(name, dark, &mut Vec::new())
    }

    fn eval_name(&self, name: &str, dark: bool, stack: &mut Vec<String>) -> Result<Rgba> {
        if stack.iter().any(|entry| entry == name) {
            bail!("color token cycle: {} -> {name}", stack.join(" -> "));
        }
        let expression = self
            .raw
            .get(name)
            .with_context(|| format!("unknown color token `{name}`"))?;
        stack.push(name.to_string());
        let result = self.eval_expression(expression, dark, stack);
        stack.pop();
        result
    }

    fn eval_expression(
        &self,
        expression: &str,
        dark: bool,
        stack: &mut Vec<String>,
    ) -> Result<Rgba> {
        let expression = expression.trim();
        if let Some(rest) = dark_condition(expression) {
            let (dark_value, light_value) = split_ternary(rest)?;
            let branch = if dark { dark_value } else { light_value };
            return self.eval_expression(branch, dark, stack);
        }
        if expression.starts_with('#') {
            return Rgba::parse(expression);
        }
        if expression == "transparent" {
            return Ok(Rgba::TRANSPARENT);
        }
        let is_reference = !expression.is_empty()
            && expression
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_');
        if is_reference {
            return self.eval_name(expression, dark, stack);
        }
        bail!("unsupported color expression `{expression}`")
    }
}

/// `dark ? <dark value> : <light value>`, with the leading `dark ?` stripped.
fn dark_condition(expression: &str) -> Option<&str> {
    let rest = expression.strip_prefix("dark")?.trim_start();
    rest.strip_prefix('?').map(str::trim_start)
}

/// Split `a : b` at the `:` belonging to the outer conditional, ignoring
/// nested `?:` pairs (the theme file only uses flat conditionals today).
fn split_ternary(expression: &str) -> Result<(&str, &str)> {
    let mut depth = 0usize;
    for (index, ch) in expression.char_indices() {
        match ch {
            '?' => depth += 1,
            ':' if depth == 0 => {
                let (left, right) = expression.split_at(index);
                return Ok((left.trim(), right[1..].trim()));
            }
            ':' => depth -= 1,
            _ => {}
        }
    }
    bail!("conditional color expression without `:`")
}

// ----------------------------------------------------------------------- checks

/// Which WCAG threshold a combination must satisfy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    /// Body-size text.
    BodyText,
    /// Icon glyphs and large text.
    IconOrLarge,
}

impl Class {
    fn threshold(self) -> f64 {
        match self {
            Self::BodyText => 4.5,
            Self::IconOrLarge => 3.0,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::BodyText => "body-size text",
            Self::IconOrLarge => "icons / large text",
        }
    }
}

/// One foreground/background token combination to verify.
#[derive(Clone, Copy, Debug)]
struct Check {
    fg: &'static str,
    bg: &'static str,
    class: Class,
}

/// Surfaces that carry body text (including dialogs/menus).
const TEXT_SURFACES: &[&str] = &[
    "window-background",
    "layer-fill",
    "card-fill",
    "panel-background",
    "flyout-background",
];
/// The three surfaces named by the design language for inline status/colors.
const CORE_SURFACES: &[&str] = &["window-background", "layer-fill", "card-fill"];
/// Text tokens; these also inherit into 20px glyphs, which the stricter text
/// threshold covers.
const TEXT_FOREGROUNDS: &[&str] = &["text-primary", "text-secondary", "text-tertiary"];
/// Status colors label connection/progress/error text inline.
const STATUS_FOREGROUNDS: &[&str] = &["success", "caution", "critical"];
/// Foregrounds painted as 20px icons (non-text contrast, 3:1).
const ICON_FOREGROUNDS: &[&str] = &[
    "text-primary",
    "text-secondary",
    "text-tertiary",
    "accent",
    "success",
    "caution",
    "critical",
];

/// The token combinations the design language promises to keep accessible.
fn checks() -> Vec<Check> {
    let mut checks = Vec::new();
    let mut body = |fg: &'static str, bg: &'static str| {
        checks.push(Check {
            fg,
            bg,
            class: Class::BodyText,
        });
    };
    for &fg in TEXT_FOREGROUNDS {
        for &bg in TEXT_SURFACES {
            body(fg, bg);
        }
    }
    // Labels on accent fills (buttons) and accent used as text/link color.
    for bg in ["accent", "accent-hover", "accent-pressed"] {
        body("accent-text", bg);
    }
    for &bg in TEXT_SURFACES {
        body("accent", bg);
    }
    body("accent", "accent-soft");
    body("text-primary", "accent-soft");
    body("text-primary", "selection-fill");
    // Inline status text.
    for &fg in STATUS_FOREGROUNDS {
        for &bg in CORE_SURFACES {
            body(fg, bg);
        }
    }
    body("attention-text", "attention-background");
    // The same foregrounds paint 20px glyphs (WCAG non-text contrast).
    for &fg in ICON_FOREGROUNDS {
        for &bg in CORE_SURFACES {
            checks.push(Check {
                fg,
                bg,
                class: Class::IconOrLarge,
            });
        }
    }
    checks
}

/// The measured result for one combination.
struct Outcome {
    check: Check,
    /// Raw token values as written in the theme file.
    fg_raw: Rgba,
    bg_raw: Rgba,
    /// Effective colors after compositing over `window-background`.
    fg: Rgba,
    bg: Rgba,
    ratio: f64,
    passed: bool,
}

/// Resolve every combination for one theme; translucent tokens are composited
/// over `window-background`, text on top of the surface it sits on.
fn evaluate(tokens: &Tokens, dark: bool, checks: &[Check]) -> Result<Vec<Outcome>> {
    let base = tokens
        .resolve("window-background", dark)
        .context("theme must define `window-background`")?;
    let mut outcomes = Vec::with_capacity(checks.len());
    for &check in checks {
        let fg_raw = tokens.resolve(check.fg, dark)?;
        let bg_raw = tokens.resolve(check.bg, dark)?;
        let bg = bg_raw.over(base);
        let fg = fg_raw.over(bg);
        let ratio = contrast_ratio(fg, bg);
        outcomes.push(Outcome {
            check,
            fg_raw,
            bg_raw,
            fg,
            bg,
            ratio,
            passed: ratio >= check.class.threshold(),
        });
    }
    Ok(outcomes)
}

// ---------------------------------------------------------------------- report

fn report_theme(
    label: &str,
    file: &Path,
    tokens: &Tokens,
    dark: bool,
    checks: &[Check],
    verbose: bool,
) -> Result<usize> {
    let outcomes = evaluate(tokens, dark, checks)?;
    let failures = outcomes.iter().filter(|outcome| !outcome.passed).count();

    println!();
    println!("== {label} ==  {}", file.display());
    print_surfaces(tokens, dark, checks)?;
    for class in [Class::BodyText, Class::IconOrLarge] {
        let rows: Vec<&Outcome> = outcomes
            .iter()
            .filter(|outcome| outcome.check.class == class)
            .collect();
        report_class(&rows, class, verbose);
    }
    println!();
    println!("  summary: {} checks, {failures} failed", outcomes.len());
    Ok(failures)
}

/// Effective value of every surface a checked combination is built on.
fn print_surfaces(tokens: &Tokens, dark: bool, checks: &[Check]) -> Result<()> {
    let base = tokens.resolve("window-background", dark)?;
    let mut names: Vec<&'static str> = Vec::new();
    for check in checks {
        if !names.contains(&check.bg) {
            names.push(check.bg);
        }
    }
    let width = names.iter().map(|name| name.len()).max().unwrap_or(0);
    println!("  surfaces (translucent tokens composited over window-background):");
    for name in names {
        let raw = tokens.resolve(name, dark)?;
        if raw.a == 255 || name == "window-background" {
            println!("    {name:<width$}  {}", raw.token_hex());
        } else {
            println!(
                "    {name:<width$}  {}  ({} over {})",
                raw.over(base).rgb_hex(),
                raw.token_hex(),
                base.rgb_hex()
            );
        }
    }
    Ok(())
}

fn report_class(rows: &[&Outcome], class: Class, verbose: bool) {
    let failed = rows.iter().filter(|outcome| !outcome.passed).count();
    let labels: Vec<String> = rows
        .iter()
        .map(|outcome| format!("{} on {}", outcome.check.fg, outcome.check.bg))
        .collect();
    let width = labels.iter().map(String::len).max().unwrap_or(0);

    println!();
    println!(
        "  {} (require >= {:.2}:1): {} checks, {failed} failed",
        class.label(),
        class.threshold(),
        rows.len()
    );
    for (outcome, label) in rows.iter().zip(&labels) {
        if outcome.passed && !verbose && class != Class::BodyText {
            continue;
        }
        let state = if outcome.passed { "PASS" } else { "FAIL" };
        println!("    {state}  {label:<width$}  {:>6.2}:1", outcome.ratio);
        if !outcome.passed {
            println!(
                "          fg {} {}; bg {} {}; required >= {:.2}:1",
                outcome.check.fg,
                describe_color(outcome.fg_raw, outcome.fg),
                outcome.check.bg,
                describe_color(outcome.bg_raw, outcome.bg),
                class.threshold(),
            );
        }
    }
}

fn describe_color(raw: Rgba, effective: Rgba) -> String {
    if raw.a == 255 {
        raw.token_hex()
    } else {
        format!("{} -> {}", raw.token_hex(), effective.rgb_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const THEME_FIXTURE: &str = r#"
export global Theme {
    in-out property <color> accent-light: #005FB8;
    in-out property <color> accent-dark: #60CDFF;
    out property <color> accent: dark ? accent-dark : accent-light;
    out property <color> window-background: dark ? #202020 : #F3F3F3;
    out property <color> scrim: #00000088;
    // out property <color> commented-out: #FF0000;
}
"#;

    #[test]
    fn parses_six_and_eight_digit_colors() {
        assert_eq!(
            Rgba::parse("#FFFFFF").unwrap(),
            Rgba {
                r: 255,
                g: 255,
                b: 255,
                a: 255
            }
        );
        assert_eq!(
            Rgba::parse("#00000072").unwrap(),
            Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 0x72
            }
        );
        assert!(Rgba::parse("#FFF").is_err());
        assert!(Rgba::parse("#GGGGGG").is_err());
        assert!(Rgba::parse("white").is_err());
    }

    #[test]
    fn composites_translucent_colors() {
        let black = Rgba::parse("#000000").unwrap();
        assert_eq!(Rgba::parse("#FFFFFF00").unwrap().over(black), black);
        let half = Rgba::parse("#FFFFFF80").unwrap().over(black);
        assert!((i32::from(half.r) - 128).abs() <= 1);
        assert_eq!(half.a, 255);
        let white = Rgba::parse("#FFFFFF").unwrap();
        assert_eq!(white.over(black), white);
    }

    #[test]
    fn wcag_luminance_and_contrast_match_reference_values() {
        let white = Rgba::parse("#FFFFFF").unwrap();
        let black = Rgba::parse("#000000").unwrap();
        assert!((luminance(white) - 1.0).abs() < 1e-9);
        assert!(luminance(black).abs() < 1e-9);
        assert!((contrast_ratio(white, black) - 21.0).abs() < 1e-9);
        assert!((contrast_ratio(black, white) - 21.0).abs() < 1e-9);
    }

    #[test]
    fn parses_theme_tokens_with_aliases_and_conditions() {
        let tokens = Tokens::parse(THEME_FIXTURE).unwrap();
        assert_eq!(
            tokens.resolve("accent", false).unwrap(),
            Rgba::parse("#005FB8").unwrap()
        );
        assert_eq!(
            tokens.resolve("accent", true).unwrap(),
            Rgba::parse("#60CDFF").unwrap()
        );
        assert_eq!(
            tokens.resolve("scrim", false).unwrap(),
            Rgba::parse("#00000088").unwrap()
        );
        assert_eq!(
            tokens.resolve("scrim", true).unwrap(),
            Rgba::parse("#00000088").unwrap()
        );
        assert!(tokens.resolve("missing", false).is_err());
    }

    #[test]
    fn rejects_color_token_cycles() {
        let cyclic = r#"
export global Theme {
    out property <color> a: b;
    out property <color> b: dark ? a : a;
}
"#;
        let tokens = Tokens::parse(cyclic).unwrap();
        let error = tokens.resolve("a", false).unwrap_err().to_string();
        assert!(error.contains("cycle"), "{error}");
    }

    #[test]
    fn flags_combinations_below_the_threshold() {
        let bad = r#"
export global Theme {
    out property <color> window-background: #FFFFFF;
    out property <color> pane: #F0F0F0;
    out property <color> text-primary: #F0F0F0;
}
"#;
        let tokens = Tokens::parse(bad).unwrap();
        let body = [Check {
            fg: "text-primary",
            bg: "pane",
            class: Class::BodyText,
        }];
        let outcomes = evaluate(&tokens, false, &body).unwrap();
        assert_eq!(outcomes.len(), 1);
        assert!(!outcomes[0].passed, "identical colors must fail 4.5:1");
        assert!(outcomes[0].ratio < 1.2);

        let icon = [Check {
            fg: "text-primary",
            bg: "pane",
            class: Class::IconOrLarge,
        }];
        assert!(!evaluate(&tokens, false, &icon).unwrap()[0].passed);
    }

    #[test]
    fn tracks_the_repository_theme_completely() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask has a parent directory");
        let source =
            fs::read_to_string(root.join(DEFAULT_THEME_FILE)).expect("ui/theme.slint is readable");
        let tokens = Tokens::parse(&source).expect("theme tokens parse");
        let checks = checks();
        assert!(checks.len() >= 50, "check list unexpectedly small");
        for dark in [false, true] {
            let outcomes = evaluate(&tokens, dark, &checks).expect("all combinations resolve");
            assert_eq!(outcomes.len(), checks.len());
            for outcome in &outcomes {
                assert!(outcome.ratio >= 1.0, "ratio below 1 is impossible");
            }
        }
    }
}
