pub fn validate_instance_name(name: &str) -> Result<(), String> {
    validate_runtime_name(name, "instance")
}

pub fn validate_env_name(name: &str) -> Result<(), String> {
    validate_runtime_name(name, "env")
}

fn validate_runtime_name(name: &str, kind: &str) -> Result<(), String> {
    if name.is_empty() || !name.chars().all(is_allowed_name_char) {
        return Err(format!(
            "invalid {kind} name '{name}': expected [A-Za-z0-9_-]+"
        ));
    }
    Ok(())
}

fn is_allowed_name_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
}

#[cfg(test)]
mod tests {
    use super::{validate_env_name, validate_instance_name};

    #[test]
    fn accepts_alnum_underscore_dash_names() {
        assert!(validate_instance_name("inst_1-a").is_ok());
        assert!(validate_env_name("env-ALPHA_2").is_ok());
    }

    #[test]
    fn rejects_invalid_name_characters() {
        for name in ["", "inst:a", "env.main", "two words", "name/segment"] {
            assert!(
                validate_instance_name(name).is_err(),
                "instance name should be rejected: {name}"
            );
            assert!(
                validate_env_name(name).is_err(),
                "env name should be rejected: {name}"
            );
        }
    }
}
