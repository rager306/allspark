//! Levenshtein edit distance: the reference scalar version, the
//! within-a-bound version, and the char-slice core both share.

#[allow(
    clippy::suspicious_operation_groupings,
    reason = "paired index bounds are intentional"
)]
pub(super) fn levenshtein_chars(a: &[char], b: &[char]) -> usize {
    if a == b {
        return 0;
    }
    let mut start = 0;
    let shared_limit = a.len().min(b.len());
    while start < shared_limit && a[start] == b[start] {
        start += 1;
    }
    let mut a_end = a.len();
    let mut b_end = b.len();
    while a_end > start && b_end > start && a[a_end - 1] == b[b_end - 1] {
        a_end -= 1;
        b_end -= 1;
    }
    let mut longer = &a[start..a_end];
    let mut shorter = &b[start..b_end];
    if longer.is_empty() {
        return shorter.len();
    }
    if shorter.is_empty() {
        return longer.len();
    }
    if shorter.len() > longer.len() {
        std::mem::swap(&mut longer, &mut shorter);
    }

    let mut row: Vec<usize> = (0..=shorter.len()).collect();
    for (line, &a_char) in longer.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = line + 1;
        for (column, &b_char) in shorter.iter().enumerate() {
            let cell = column + 1;
            let above = row[cell];
            row[cell] = if a_char == b_char {
                diagonal
            } else {
                (above + 1).min(row[cell - 1] + 1).min(diagonal + 1)
            };
            diagonal = above;
        }
    }
    row[shorter.len()]
}

/// Levenshtein edit distance over Unicode scalar values.
///
/// The TypeScript source used UTF-16 code units. Rust deliberately uses
/// Unicode scalar values, so astral characters count as one element.
pub fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    levenshtein_chars(&a_chars, &b_chars)
}

/// [`levenshtein_distance`] over pre-collected chars, or `None` above `max`.
///
/// Only the `2·max + 1` diagonals that can stay within `max` are computed,
/// so the cost is O(len·max) instead of O(len²).
pub fn levenshtein_within(a: &[char], b: &[char], max: usize) -> Option<usize> {
    if a == b {
        return Some(0);
    }
    let shared = a
        .iter()
        .zip(b)
        .take_while(|(left, right)| left == right)
        .count();
    let (a, b) = (&a[shared..], &b[shared..]);
    let shared = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    let (a, b) = (&a[..a.len() - shared], &b[..b.len() - shared]);
    let (longer, shorter) = if a.len() >= b.len() { (a, b) } else { (b, a) };
    let (rows, columns) = (longer.len(), shorter.len());
    if rows - columns > max {
        return None;
    }
    if columns == 0 {
        return Some(rows);
    }
    // No distance exceeds `rows`; capping keeps `over` from overflowing.
    let max = max.min(rows);
    let over = max + 1;
    let mut previous: Vec<usize> = (0..=columns).map(|column| column.min(over)).collect();
    let mut current = vec![over; columns + 1];
    for row in 1..=rows {
        let low = row.saturating_sub(max).max(1);
        let high = (row + max).min(columns);
        current[0] = row.min(over);
        if low > 1 {
            current[low - 1] = over;
        }
        let mut row_min = current[0];
        let character = longer[row - 1];
        for column in low..=high {
            let substitute = previous[column - 1] + usize::from(character != shorter[column - 1]);
            let value = substitute
                .min(previous[column] + 1)
                .min(current[column - 1] + 1)
                .min(over);
            current[column] = value;
            row_min = row_min.min(value);
        }
        if high < columns {
            current[high + 1] = over;
        }
        if row_min > max {
            return None;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let distance = previous[columns];
    (distance <= max).then_some(distance)
}
