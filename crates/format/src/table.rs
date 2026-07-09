/// Format data as an ASCII table with pipe separators and auto-sized columns.
///
/// Returns the formatted table as a String.
pub fn format_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    if headers.is_empty() {
        return String::new();
    }

    let col_count = headers.len();

    // Calculate column widths from headers and all rows
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();

    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < col_count {
                widths[i] = widths[i].max(cell.len());
            }
        }
    }

    let mut output = String::new();

    // Top border
    output.push_str(&format_border(&widths));
    output.push('\n');

    // Header row
    output.push_str(&format_row(headers, &widths));
    output.push('\n');

    // Header separator
    output.push_str(&format_separator(&widths));
    output.push('\n');

    // Data rows
    for row in rows {
        // Pad row to match column count if needed
        let padded: Vec<String> = (0..col_count)
            .map(|i| {
                if i < row.len() {
                    row[i].clone()
                } else {
                    String::new()
                }
            })
            .collect();
        let cells: Vec<&str> = padded.iter().map(|s| s.as_str()).collect();
        output.push_str(&format_row(&cells, &widths));
        output.push('\n');
    }

    // Bottom border
    output.push_str(&format_border(&widths));

    output
}

fn format_border(widths: &[usize]) -> String {
    let parts: Vec<String> = widths.iter().map(|w| "-".repeat(w + 2)).collect();
    format!("+{}+", parts.join("+"))
}

fn format_separator(widths: &[usize]) -> String {
    let parts: Vec<String> = widths.iter().map(|w| "-".repeat(w + 2)).collect();
    format!("+{}+", parts.join("+"))
}

fn format_row(cells: &[&str], widths: &[usize]) -> String {
    let parts: Vec<String> = cells
        .iter()
        .enumerate()
        .map(|(i, cell)| format!(" {:<width$} ", cell, width = widths[i]))
        .collect();
    format!("|{}|", parts.join("|"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_table() {
        let headers = &["Name", "Age", "City"];
        let rows = &[
            vec!["Alice".to_string(), "30".to_string(), "NYC".to_string()],
            vec!["Bob".to_string(), "25".to_string(), "LA".to_string()],
        ];
        let result = format_table(headers, rows);
        assert!(result.contains("| Name  | Age | City |"));
        assert!(result.contains("| Alice | 30  | NYC  |"));
        assert!(result.contains("| Bob   | 25  | LA   |"));
    }

    #[test]
    fn test_empty_rows() {
        let headers = &["Col"];
        let result = format_table(headers, &[]);
        assert!(result.contains("| Col |"));
    }

    #[test]
    fn test_empty_headers() {
        let result = format_table(&[], &[]);
        assert_eq!(result, "");
    }
}
