use super::*;

#[test]
fn compiles_log_format_escape_modes() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                log_format json escape=json '{"request":"$request"}';
                log_format plain escape=none '$request';
                log_format legacy '$request';
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let formats = &config.http.as_ref().unwrap().log_formats;
    assert_eq!(formats["json"].escape, LogEscapeMode::Json);
    assert_eq!(formats["json"].template, r#"{"request":"$request"}"#);
    assert_eq!(formats["plain"].escape, LogEscapeMode::None);
    assert_eq!(formats["legacy"].escape, LogEscapeMode::Default);

    fs::write(
        &config_path,
        "events {} http { log_format invalid escape=yaml '$request'; }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path).unwrap_err();
    assert!(
        error
            .message
            .contains("invalid escape parameter \"escape=yaml\""),
        "{error}"
    );
    let _ = fs::remove_dir_all(prefix);
}
