/// Always prints, regardless of debug state.
#[macro_export]
macro_rules! always_print {
    ($($arg:tt)*) => {
        {
            let line = format!($($arg)*);
            println!("{}", line);
            $crate::utils::logs::push(&line);
        }
    };
}

/// Always prints to stderr, regardless of debug state.
#[macro_export]
macro_rules! always_eprint {
    ($($arg:tt)*) => {
        {
            let line = format!($($arg)*);
            eprintln!("{}", line);
            $crate::utils::logs::push(&line);
        }
    };
}

#[cfg(test)]
mod tests {
    /// Buffer must receive logs — release builds have nowhere else to look.
    #[test]
    fn always_print_reaches_the_ring_buffer() {
        // serialized with other buffer tests to avoid eviction
        let _guard = crate::utils::logs::buffer_test_guard();

        let before = crate::utils::logs::generation();
        crate::always_print!("logger test marker {}", 42);
        assert!(
            crate::utils::logs::generation() > before,
            "a logged line must land in the buffer"
        );

        let recent = crate::utils::logs::recent(50).join("\n");
        assert!(recent.contains("logger test marker 42"), "{}", recent);
    }

    /// Arguments must be evaluated exactly once.
    #[test]
    fn arguments_are_evaluated_exactly_once() {
        let mut calls = 0;
        let mut bump = || {
            calls += 1;
            calls
        };
        crate::always_print!("evaluated {} time(s)", bump());
        assert_eq!(calls, 1, "a side-effecting argument must run once");
    }
}
