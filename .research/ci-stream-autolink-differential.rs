mod old {
pub(crate) fn bare_candidate(bytes: &[char], i: usize, line_has_at: bool) -> bool {
    for (prefix, ci) in [
        ("https://", true),
        ("http://", true),
        ("ftp://", true),
        ("www.", false),
    ] {
        if starts_with(bytes, i, prefix, ci) {
            return true;
        }
    }
    if line_has_at && (i == 0 || !is_local_part(bytes[i - 1])) {
        // Only the first character of a local-part run can start an email match, and one
        // scan per run keeps a long local-part run linear instead of quadratic.
        let mut j = i;
        while j < bytes.len() && is_local_part(bytes[j]) {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == '@' {
            return true;
        }
    }
    false
}

fn is_local_part(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-')
}

fn starts_with(bytes: &[char], i: usize, prefix: &str, ignore_case: bool) -> bool {
    prefix.chars().enumerate().all(|(k, pc)| {
        bytes.get(i + k).is_some_and(|&c| {
            if ignore_case {
                c.eq_ignore_ascii_case(&pc)
            } else {
                c == pc
            }
        })
    })
}

}
mod new {
pub(crate) fn bare_candidate(bytes: &[char], i: usize, line_has_at: bool) -> bool {
    let protocol = match bytes.get(i) {
        Some('h' | 'H') => {
            starts_with(bytes, i, "https://", true) || starts_with(bytes, i, "http://", true)
        }
        Some('f' | 'F') => starts_with(bytes, i, "ftp://", true),
        Some('w') => starts_with(bytes, i, "www.", false),
        _ => false,
    };
    if protocol {
        return true;
    }
    if line_has_at && (i == 0 || !is_local_part(bytes[i - 1])) {
        // Only the first character of a local-part run can start an email match, and one
        // scan per run keeps a long local-part run linear instead of quadratic.
        let mut j = i;
        while j < bytes.len() && is_local_part(bytes[j]) {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == '@' {
            return true;
        }
    }
    false
}

fn is_local_part(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-')
}

fn starts_with(bytes: &[char], i: usize, prefix: &str, ignore_case: bool) -> bool {
    prefix.chars().enumerate().all(|(k, pc)| {
        bytes.get(i + k).is_some_and(|&c| {
            if ignore_case {
                c.eq_ignore_ascii_case(&pc)
            } else {
                c == pc
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::bare_candidate;

    #[test]
    fn candidates_preserve_scheme_case_and_email_run_boundaries() {
        for (text, expected) in [
            ("history history", vec![]),
            ("HTTPS://a HTTP://b ftp://c www.d", vec![0, 10, 19, 27]),
            ("WWW.a Ftp://b https:/c", vec![6]),
            ("prefix a.b+c-d@host.test suffix", vec![7]),
            ("www.user@host.test", vec![0]),
        ] {
            let chars: Vec<char> = text.chars().collect();
            let actual: Vec<usize> = (0..chars.len())
                .filter(|&i| bare_candidate(&chars, i, chars.contains(&'@')))
                .collect();
            assert_eq!(actual, expected, "{text}");
        }
    }
}

}
fn main() {
 let parts = ["h", "H", "f", "F", "w", "W", "history ", "https://", "HTTP://", "ftp://", "www.", "WWW.", "@", ".", "a+b-c_d", "界", " "];
 let mut checked=0;
 for a in parts { for b in parts { for c in parts {
 let corpus=format!("{a}{b}{c}"); let chars: Vec<char>=corpus.chars().collect(); let at=chars.contains(&'@');
 for i in 0..chars.len() { assert_eq!(old::bare_candidate(&chars,i,at), new::bare_candidate(&chars,i,at), "{corpus} offset{i}"); checked+=1; }
 }}}
 println!("Equivalent candidate results across {checked} positions in4913 synthetic mixed Unicode/scheme/email strings");
}
