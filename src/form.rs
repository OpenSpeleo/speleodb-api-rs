use crate::HttpError;

pub(crate) fn multipart_scalar(
    value: &impl serde::Serialize,
) -> Result<reqwest::multipart::Part, HttpError> {
    let text = match serde_json::to_value(value).map_err(HttpError::serialization_error)? {
        serde_json::Value::String(value) => value,
        serde_json::Value::Number(value) => value.to_string(),
        serde_json::Value::Bool(value) => value.to_string(),
        _ => return Err(HttpError::Serialization("Multipart scalar must be text, a number or a boolean; use the explicit empty-string alternative to clear a supported field".into())),
    };
    Ok(reqwest::multipart::Part::text(text))
}

/// Text-only open form objects retain their typed additional fields on wire.
pub(crate) fn multipart_text_fields(
    request: &impl serde::Serialize,
) -> Result<reqwest::multipart::Form, HttpError> {
    let value = serde_json::to_value(request).map_err(HttpError::serialization_error)?;
    let serde_json::Value::Object(fields) = value else {
        return Err(HttpError::Config(
            "Multipart text request must be an object".into(),
        ));
    };
    let mut form = reqwest::multipart::Form::new();
    for (name, value) in fields {
        let serde_json::Value::String(value) = value else {
            return Err(HttpError::Config(
                "Multipart text fields must be strings".into(),
            ));
        };
        form = form.text(name, value);
    }
    Ok(form)
}
