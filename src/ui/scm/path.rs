//! Turning a repo-relative path and a timestamp into something that fits in a
//! 260px column. All pure, all cheap, all unit-tested — the panel calls these
//! once per visible row per frame.

/// Split `src/ui/app.rs` into `("app.rs", "src/ui")`.
///
/// The panel renders these as two runs with different sizes and colours, so
/// they have to come back as separate slices rather than one pre-joined
/// string. A path with no directory gets an empty second half.
pub(crate) fn split_display_path(rel: &str) -> (&str, &str) {
    // A trailing slash means the caller handed us a directory; the last
    // component is still the name, so drop the slash before splitting.
    let trimmed = rel.strip_suffix('/').unwrap_or(rel);
    match trimmed.rsplit_once('/') {
        Some((dir, name)) => (name, dir),
        None => (trimmed, ""),
    }
}

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
/// Calendar-average, so "12mo" and "1y" describe the same distance instead of
/// leaving a gap where 365 days is neither.
const MONTH: i64 = DAY * 30;
const YEAR: i64 = DAY * 365;

/// `"2h"` / `"3d"` / `"5mo"` — a graph row has about 26px for this.
///
/// Through the i18n table like `home::relative_time`, in the compact spelling
/// this column's width demands — "now" is still an English word, and the row,
/// the tooltip, the detail byline and the overlay header all read it.
///
/// `now` is a parameter rather than a clock read so the whole thing stays a
/// pure function, and so a test can sit exactly on a boundary.
pub(crate) fn relative_time(now_unix: i64, then_unix: i64) -> String {
    use crate::ui::i18n::{L10nKey, t, t_fmt};
    // A commit stamped in the future (clock skew across machines is routine in
    // a shared repo) reads as "now" rather than as a negative age.
    let delta = (now_unix - then_unix).max(0);
    let unit = |key: L10nKey, n: i64| t_fmt(key, &[("n", &n.to_string())]);
    match delta {
        d if d < MINUTE => t(L10nKey::ScmTimeNow).to_string(),
        d if d < HOUR => unit(L10nKey::ScmTimeMinutes, d / MINUTE),
        d if d < DAY => unit(L10nKey::ScmTimeHours, d / HOUR),
        d if d < MONTH => unit(L10nKey::ScmTimeDays, d / DAY),
        d if d < YEAR => unit(L10nKey::ScmTimeMonths, d / MONTH),
        d => unit(L10nKey::ScmTimeYears, d / YEAR),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_display_path_separates_the_name_from_its_directory() {
        assert_eq!(split_display_path("src/ui/app.rs"), ("app.rs", "src/ui"));
        assert_eq!(split_display_path("README.md"), ("README.md", ""));
        assert_eq!(split_display_path("a/b"), ("b", "a"));
        assert_eq!(split_display_path(""), ("", ""));
    }

    #[test]
    fn split_display_path_ignores_a_trailing_slash() {
        assert_eq!(split_display_path("src/ui/"), ("ui", "src"));
        assert_eq!(split_display_path("src/"), ("src", ""));
        // A leading slash leaves an empty directory half rather than dropping
        // the root — the caller decides how to render that.
        assert_eq!(split_display_path("/etc"), ("etc", ""));
    }

    #[test]
    fn relative_time_covers_every_bucket() {
        crate::ui::i18n::set_locale("en");
        let now = 1_800_000_000i64;
        let ago = |secs: i64| relative_time(now, now - secs);
        assert_eq!(ago(0), "now");
        assert_eq!(ago(59), "now");
        assert_eq!(ago(60), "1m");
        assert_eq!(ago(90), "1m");
        assert_eq!(ago(59 * MINUTE), "59m");
        assert_eq!(ago(HOUR), "1h");
        assert_eq!(ago(23 * HOUR), "23h");
        assert_eq!(ago(DAY), "1d");
        assert_eq!(ago(29 * DAY), "29d");
        assert_eq!(ago(MONTH), "1mo");
        assert_eq!(ago(YEAR - 1), "12mo");
        assert_eq!(ago(YEAR), "1y");
        assert_eq!(ago(5 * YEAR), "5y");
    }

    #[test]
    fn relative_time_clamps_commits_from_the_future() {
        crate::ui::i18n::set_locale("en");
        let now = 1_800_000_000i64;
        assert_eq!(relative_time(now, now + DAY), "now");
    }

    /// The row, the tooltip and the overlay byline all read this — it goes
    /// through the i18n table like `home::relative_time`, in compact form.
    #[test]
    fn relative_time_speaks_the_ui_language() {
        crate::ui::i18n::set_locale("zh-CN");
        let now = 1_800_000_000i64;
        assert_eq!(relative_time(now, now), "刚刚");
        assert_eq!(relative_time(now, now - 2 * HOUR), "2时");
        // “3月”会被读成月份名,所以是“个月”。
        assert_eq!(relative_time(now, now - 3 * MONTH), "3个月");
        crate::ui::i18n::set_locale("en");
    }
}
