//! Named selection from native effective developer instructions. See docs/3-accounts.md.
use toml_edit::Value;

pub fn select_character(source: &str, selected: &str) -> Result<String, String> {
    let selected = selected.trim();
    let mut sections: Vec<(Option<String>, String, String)> =
        vec![(None, String::new(), String::new())];
    let mut fence: Option<(char, usize)> = None;
    let mut names = std::collections::HashSet::new();
    for line in source.split_inclusive('\n') {
        let text = line.trim_end_matches(['\r', '\n']);
        let trimmed = text.trim_start();
        let leading = trimmed.chars().next().unwrap_or(' ');
        let count = trimmed.chars().take_while(|c| *c == leading).count();
        if matches!(leading, '`' | '~') && count >= 3 {
            match fence {
                None => fence = Some((leading, count)),
                Some((mark, length))
                    if mark == leading && count >= length && trimmed[count..].trim().is_empty() =>
                {
                    fence = None
                }
                _ => {}
            }
        } else if fence.is_none() && text.starts_with("# ") {
            let heading = text[2..].trim_end();
            if heading.starts_with("character_") {
                if heading.len() == "character_".len() || heading.chars().any(char::is_whitespace) {
                    return Err(
                        "Codex のキャラ見出しは # character_<名前> の一行で指定してください。"
                            .into(),
                    );
                }
                if !names.insert(heading.to_string()) {
                    return Err("Codex のキャラ名が重複しています。定義を確認してください。".into());
                }
                sections.push((Some(heading.to_string()), line.to_string(), String::new()));
                continue;
            }
            // Every other H1 starts common text, including an optional end heading.
            sections.push((None, String::new(), String::new()));
        }
        let section = sections.last_mut().unwrap();
        section.2.push_str(line);
    }
    if sections
        .iter()
        .any(|(name, _, body)| name.is_some() && body.trim().is_empty())
    {
        return Err("Codex のキャラ本文が空です。定義を確認してください。".into());
    }
    if !names.contains(selected) {
        return Err("Codex のキャラ名が有効な developer_instructions にありません。見出しの名前を正確に指定してください。旧保存値は追加指示として送りません。project trust・profile・-c も確認してください。".into());
    }
    Ok(sections
        .into_iter()
        .filter(|(name, _, _)| name.as_deref().is_none_or(|name| name == selected))
        .map(|(_, heading, body)| heading + &body)
        .collect())
}

/// ASCII TOML basic string. Shell expansion and CRT quote parity cannot alter its body.
pub fn instruction_value(text: &str) -> String {
    let mut encoded = String::from("\"");
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || "._:/=-".contains(c) {
            encoded.push(c);
        } else if (c as u32) <= 0xffff {
            encoded.push_str(&format!("\\u{:04x}", c as u32));
        } else {
            encoded.push_str(&format!("\\U{:08x}", c as u32));
        }
    }
    encoded.push('"');
    encoded
}
fn encode_value(value: &Value) -> String {
    match value {
        Value::String(v) => instruction_value(v.value()),
        Value::Integer(v) => v.value().to_string(),
        Value::Float(v) => {
            if v.value().is_nan() {
                "nan".into()
            } else if v.value().is_infinite() {
                if v.value().is_sign_negative() {
                    "-inf".into()
                } else {
                    "inf".into()
                }
            } else {
                let text = v.value().to_string();
                if text.contains('.') || text.contains('e') {
                    text
                } else {
                    format!("{text}.0")
                }
            }
        }
        Value::Boolean(v) => v.value().to_string(),
        Value::Datetime(v) => v.value().to_string(),
        Value::Array(v) => format!(
            "[{}]",
            v.iter().map(encode_value).collect::<Vec<_>>().join(",")
        ),
        Value::InlineTable(v) => format!(
            "{{{}}}",
            v.iter()
                .map(|(key, value)| format!("{}={}", instruction_value(key), encode_value(value)))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}
pub fn transport_options(args: &[String]) -> Result<Vec<String>, String> {
    let mut output = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if ["-p", "-C", "-c"]
            .iter()
            .any(|flag| arg.starts_with(flag) && arg.len() > 2)
        {
            return Err("Codex の短いフラグは値と分けて指定してください（-p 名前、-C パス、-c key=value）。起動を止めました。".into());
        }
        let config = if ["-c", "--config"].contains(&arg.as_str()) {
            i += 1;
            Some(
                args.get(i)
                    .ok_or("Codex -c には key=value が必要です。")?
                    .as_str(),
            )
        } else {
            arg.strip_prefix("--config=")
                .or_else(|| arg.strip_prefix("-c="))
        };
        if let Some(config) = config {
            let (key, raw) = config
                .split_once('=')
                .ok_or("Codex -c には key=value が必要です。")?;
            // The key is TOML dotted-key syntax; reject shell metacharacters rather
            // than silently rewrite a key into a different setting.
            if key.is_empty() || !super::super::console_safe(key) || key.contains('%') {
                return Err("Codex -c の設定名を起動の行へ安全に運べません。".into());
            }
            // Codex also treats an invalid TOML value as a literal string.
            let value = raw
                .parse::<Value>()
                .map(|v| encode_value(&v))
                .unwrap_or_else(|_| instruction_value(raw));
            output.extend(["-c".into(), format!("{key}={value}")]);
        } else {
            if !super::super::console_safe(arg) || arg.contains('%') {
                return Err("Codex の起動オプションを安全に運べません。オプションを省略せず、起動を止めました。".into());
            }
            output.push(arg.clone());
            if [
                "-p",
                "--profile",
                "-C",
                "--cd",
                "-m",
                "--model",
                "-a",
                "--ask-for-approval",
                "-s",
                "--sandbox",
                "--add-dir",
                "--enable",
                "--disable",
                "-i",
                "--image",
            ]
            .contains(&arg.as_str())
            {
                i += 1;
                let value = args.get(i).ok_or("Codex のオプションには値が必要です。")?;
                if !super::super::console_safe(value) || value.contains('%') {
                    return Err("Codex のオプション値を安全に運べません。起動を止めました。".into());
                }
                output.push(value.clone());
            }
        }
        i += 1;
    }
    Ok(output)
}
pub fn apply_character(args: &mut Vec<String>, source: &str, selected: &str) -> Result<(), String> {
    let body = select_character(source, selected)?;
    args.extend([
        "-c".into(),
        format!("developer_instructions={}", instruction_value(&body)),
    ]);
    Ok(())
}
pub fn check_command_length(command: &str, args: &[String]) -> Result<(), String> {
    // Includes /C, separators and the CRT quote/backslash expansion used by PTY.
    let length = command.encode_utf16().count()
        + 16
        + args
            .iter()
            .map(|arg| {
                arg.encode_utf16().count()
                    + arg.chars().filter(|c| *c == '"' || *c == '\\').count()
                    + 3
            })
            .sum::<usize>();
    if cfg!(windows) && length > 8191 {
        return Err("Codex の起動の行が Windows の8191文字制限を超えます。指示を切り詰めず起動を止めました。キャラ本文や起動オプションを短くしてください。".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_keeps_common_bytes_and_only_one_identity() {
        let source = "COMMON\r\n# character_Codex_Lin\r\nNAME=Lin\r\n## Detail\r\nLin body\r\n# character_Codex_Lay\r\nNAME=Lay\r\nLay body\r\n# Shared\r\nTAIL\r\n";
        for (name, other) in [("Lin", "Lay"), ("Lay", "Lin")] {
            let result = select_character(source, &format!("character_Codex_{name}")).unwrap();
            assert!(result.starts_with("COMMON\r\n"));
            assert!(result.ends_with("# Shared\r\nTAIL\r\n"));
            assert!(result.contains(&format!("NAME={name}\r\n")));
            assert!(!result.contains(other));
        }
    }
    #[test]
    fn fences_are_body_and_duplicates_empty_and_unknown_fail_closed() {
        let source = "# character_Lin\n```md\n# character_Fake\n```\n~~~\n# Also body\n~~~\n---\n";
        assert_eq!(select_character(source, "character_Lin").unwrap(), source);
        for bad in [
            "# character_Lin\n# End\n",
            "# character_Lin\nA\n# character_Lin\nB\n",
            "# character_\nA",
        ] {
            assert!(select_character(bad, "character_Lin").is_err());
        }
        assert!(select_character(source, "Lin").is_err());
        assert!(select_character("", "character_Lin").is_err());
    }
    #[test]
    fn text_round_trips_through_toml_without_shell_expansion() {
        let text = "日本語\r\nNAME=Lin & | () %PATH% \"quote\" \\ 😀";
        let value = instruction_value(text);
        assert!(value.is_ascii());
        assert!(!value.contains(['&', '|', '(', ')', '%', '\r', '\n']));
        assert_eq!(value.parse::<Value>().unwrap().as_str(), Some(text));
        let args = transport_options(&[
            "--profile".into(),
            "custom.v2".into(),
            "-c".into(),
            format!("developer_instructions={value}"),
            "--config=features={test='日本語',values=['&',1.0]}".into(),
        ])
        .unwrap();
        assert_eq!(&args[..2], &["--profile", "custom.v2"]);
        assert_eq!(
            args[3]
                .split_once('=')
                .unwrap()
                .1
                .parse::<Value>()
                .unwrap()
                .as_str(),
            Some(text)
        );
        assert!(args[5]
            .split_once('=')
            .unwrap()
            .1
            .parse::<Value>()
            .unwrap()
            .is_inline_table());
        assert!(transport_options(&["--cd".into(), "%PATH%".into()]).is_err());
        for flag in [
            "-pcustom",
            "-Csubdir",
            "-cdeveloper_instructions=body",
            "-p=work",
            "-C=dir",
            "-c=key=value",
        ] {
            assert!(transport_options(&[flag.into()]).is_err());
        }
        assert_eq!(
            transport_options(&["--model".into(), "-pcustom".into()]).unwrap(),
            ["--model", "-pcustom"]
        );
    }
    #[test]
    fn long_windows_lines_are_refused_and_resume_keeps_config_options() {
        if cfg!(windows) {
            assert!(
                check_command_length("codex", &[instruction_value(&"日本語".repeat(1000))])
                    .is_err()
            );
        }
        let options = vec![
            "--profile".into(),
            "work".into(),
            "-c".into(),
            "developer_instructions=\"日本語\"".into(),
        ];
        let resumed = crate::resume_launch_args(
            &["resume".into(), "native-id".into()],
            &options,
            crate::Cli::CodexCli,
        );
        assert_eq!(&resumed[2..], options);
        let encoded = transport_options(&resumed).unwrap();
        assert_eq!(&encoded[..4], &["resume", "native-id", "--profile", "work"]);
        assert_eq!(
            encoded[5]
                .split_once('=')
                .unwrap()
                .1
                .parse::<Value>()
                .unwrap()
                .as_str(),
            Some("日本語")
        );
        for raw in ["nan", "inf", "-inf", "1.0", "-0.0"] {
            assert!(encode_value(&raw.parse::<Value>().unwrap())
                .parse::<Value>()
                .unwrap()
                .is_float());
        }
    }
}
