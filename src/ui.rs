use std::io::IsTerminal;

pub fn heading(text: &str) -> String {
    if std::io::stdout().is_terminal() {
        format!("\x1b[1m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

pub fn print_key_values(rows: &[(&str, &str)]) {
    let width = rows.iter().map(|(key, _)| key.len()).max().unwrap_or(0);
    for (key, value) in rows {
        println!("{:<width$}  {}", key, value, width = width);
    }
}

pub fn print_table(headers: &[&str], rows: &[Vec<String>], right_align: &[usize]) {
    let mut widths = headers
        .iter()
        .map(|header| header.len())
        .collect::<Vec<_>>();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.len());
        }
    }

    print_separator(&widths);
    print_table_row(
        &headers
            .iter()
            .map(|header| (*header).to_string())
            .collect::<Vec<_>>(),
        &widths,
        right_align,
    );
    print_separator(&widths);
    for row in rows {
        print_table_row(row, &widths, right_align);
    }
    print_separator(&widths);
}

fn print_separator(widths: &[usize]) {
    print!("+");
    for width in widths {
        print!("{}+", "-".repeat(width + 2));
    }
    println!();
}

fn print_table_row(row: &[String], widths: &[usize], right_align: &[usize]) {
    print!("|");
    for (index, cell) in row.iter().enumerate() {
        let width = widths[index];
        if right_align.contains(&index) {
            print!(" {:>width$} |", cell, width = width);
        } else {
            print!(" {:<width$} |", cell, width = width);
        }
    }
    println!();
}
