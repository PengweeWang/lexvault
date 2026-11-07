/// Normalizes a headword using ECDICT's `sw` convention.
pub fn normalize_query(query: &str) -> String {
    query
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::normalize_query;

    #[test]
    fn removes_punctuation_and_lowercases() {
        assert_eq!(normalize_query("Long-Time"), "longtime");
        assert_eq!(normalize_query(" E-mail "), "email");
        assert_eq!(normalize_query("Über"), "über");
    }
}
