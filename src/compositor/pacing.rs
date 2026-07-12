use std::sync::OnceLock;

pub(crate) fn client_pacing_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("TYPHON_FRAME_PACING_DEBUG")
            .ok()
            .is_some_and(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on" | "debug" | "trace"
                )
            })
    })
}

pub(crate) fn client_pacing_now_ns() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
        return 0;
    }
    u64::try_from(time.tv_sec)
        .unwrap_or(0)
        .saturating_mul(1_000_000_000)
        .saturating_add(u64::try_from(time.tv_nsec).unwrap_or(0))
}

pub(crate) fn client_pacing_log(event: &str, fields: &[(&str, String)]) {
    if client_pacing_enabled() {
        println!(
            "{}",
            client_pacing_line(event, client_pacing_now_ns(), fields)
        );
    }
}

pub(crate) fn client_pacing_line(event: &str, event_ns: u64, fields: &[(&str, String)]) -> String {
    let mut line = format!("typhon pacing: event={event} event_ns={event_ns}");
    for (key, value) in fields {
        line.push(' ');
        line.push_str(key);
        line.push('=');
        line.push_str(&pacing_value(value));
    }
    line
}

fn pacing_value(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./:@+=".contains(c))
    {
        return value.to_string();
    }
    format!("\"{}\"", value.replace('\\', "\\\\").replace('\"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_pacing_line_has_stable_prefix_event_and_timestamp() {
        assert_eq!(
            client_pacing_line(
                "surface_commit",
                42,
                &[("surface", "7".to_string()), ("damage", "true".to_string())],
            ),
            "typhon pacing: event=surface_commit event_ns=42 surface=7 damage=true"
        );
    }
}
