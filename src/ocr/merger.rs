pub fn merge_pages(mut pages: Vec<(u32, String)>) -> String {
    pages.sort_by_key(|(page, _)| *page);
    pages
        .into_iter()
        .map(|(page, markdown)| format!("<!-- Page {page} -->\n\n{}", markdown.trim()))
        .collect::<Vec<_>>()
        .join("\n\n---\n\n")
}

#[cfg(test)]
mod tests {
    use super::merge_pages;

    #[test]
    fn sorts_and_separates_pages() {
        let result = merge_pages(vec![(2, "second".into()), (1, "first".into())]);
        assert_eq!(
            result,
            "<!-- Page 1 -->\n\nfirst\n\n---\n\n<!-- Page 2 -->\n\nsecond"
        );
    }
}
