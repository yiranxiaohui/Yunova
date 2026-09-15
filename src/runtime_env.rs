//! Yunova configuration names take precedence over the pre-rename aliases.

pub fn var(name: &str) -> Result<String, std::env::VarError> {
    value_with(name, |key| std::env::var(key).ok()).ok_or(std::env::VarError::NotPresent)
}

pub fn value_with<F>(name: &str, mut getenv: F) -> Option<String>
where
    F: FnMut(&str) -> Option<String>,
{
    getenv(name).or_else(|| {
        name.strip_prefix("YUNOVA_")
            .and_then(|suffix| getenv(&format!("NOVACHAT_{suffix}")))
    })
}

#[cfg(test)]
mod tests {
    use super::value_with;

    #[test]
    fn new_names_override_legacy_server_configuration() {
        for suffix in ["BIND", "DATABASE_URL", "S3_BUCKET", "DATA_DIR"] {
            let name = format!("YUNOVA_{suffix}");
            let legacy = format!("NOVACHAT_{suffix}");
            let get = |key: &str| match key {
                key if key == name => Some("new".to_string()),
                key if key == legacy => Some("old".to_string()),
                _ => None,
            };
            assert_eq!(value_with(&name, get).as_deref(), Some("new"));
            assert_eq!(
                value_with(&name, |key| (key == legacy).then(|| "old".into())).as_deref(),
                Some("old")
            );
        }
    }

    #[test]
    fn unrelated_environment_names_have_no_alias() {
        let mut requested = Vec::new();
        assert!(
            value_with("AWS_REGION", |key| {
                requested.push(key.to_string());
                None
            })
            .is_none()
        );
        assert_eq!(requested, ["AWS_REGION"]);
    }
}
