//! RFEM-style object lists such as `1-4,7 9`.

/// Parses a list like `1-4,7 9` (commas and/or spaces, ranges with `-`).
/// Returns `None` if any part is not a number or range.
pub fn parse_list(text: &str) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    for part in text.split([',', ' ', ';']).filter(|p| !p.is_empty()) {
        match part.split_once('-') {
            Some((a, b)) => {
                let (a, b): (u32, u32) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
                if a <= b {
                    out.extend(a..=b);
                } else {
                    out.extend((b..=a).rev());
                }
            }
            None => out.push(part.trim().parse().ok()?),
        }
    }
    Some(out)
}

/// Formats numbers compactly, merging ascending runs of three or more: `[1,2,3,4,7]` -> `1-4,7`.
pub fn format_list(items: &[u32]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < items.len() {
        let mut j = i;
        while j + 1 < items.len() && items[j + 1] == items[j] + 1 {
            j += 1;
        }
        if !out.is_empty() {
            out.push(',');
        }
        if j >= i + 2 {
            out.push_str(&format!("{}-{}", items[i], items[j]));
            i = j + 1;
        } else {
            out.push_str(&items[i].to_string());
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        assert_eq!(parse_list("1-4,7 9").unwrap(), vec![1, 2, 3, 4, 7, 9]);
        assert_eq!(format_list(&[1, 2, 3, 4, 7, 8]), "1-4,7,8");
        assert_eq!(parse_list(""), Some(vec![]));
        assert_eq!(parse_list("a"), None);
    }
}
