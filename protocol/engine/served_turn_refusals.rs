//! The backend's refusals of a turn, read where the client declares them.
//!
//! The client declares each refusal once, as a row of `SERVED_TURN_REFUSALS`
//! in `packages/core/src/core/generation-refusal.ts`: the type the backend's
//! in-stream error carries, why the turn was refused, the name the client
//! records the failed response under, and the notice its redraw is told. The
//! certifier reads a refusal by the first and the third, so the build reads
//! them from that declaration, in the file the reviewed client patch writes,
//! and the certifier has no list of its own. The declaration is read only in
//! its own shape; anything else stops the build, naming what it found.

/// The reviewed client patch, from the engine's crate directory.
pub(crate) const PATCH: &str = "../../patches/qwen-code-0.21.12-agent-service.patch";

/// The client file that declares the refusals.
const FILE: &str = "packages/core/src/core/generation-refusal.ts";

const OPEN: &str = "export const SERVED_TURN_REFUSALS = [";
const CLOSE: &str = "] as const;";

/// Each refusal's wire type and recorded name, in declared order.
pub(crate) fn read(patch: &str) -> Result<Vec<(String, String)>, String> {
    let header = format!("diff --git a/{FILE} b/{FILE}\n");
    let starts: Vec<usize> = patch
        .match_indices(&header)
        .map(|(at, _)| at)
        .filter(|at| *at == 0 || patch.as_bytes()[at - 1] == b'\n')
        .collect();
    if starts.len() != 1 {
        return Err(format!(
            "the reviewed client patch writes {FILE} {} times, where the client declares the backend's refusals of a turn once",
            starts.len()
        ));
    }
    let section = &patch[starts[0] + header.len()..];
    let section = section
        .find("\ndiff --git ")
        .map_or(section, |end| &section[..end]);
    // The file's lines as the patch leaves them: what its hunks keep and add.
    let mut lines = Vec::new();
    let mut in_hunk = false;
    for line in section.lines() {
        if line.starts_with("@@ ") {
            in_hunk = true;
        } else if in_hunk {
            if let Some(kept) = line.strip_prefix('+').or_else(|| line.strip_prefix(' ')) {
                lines.push(kept);
            }
        }
    }
    let opens: Vec<usize> = (0..lines.len()).filter(|at| lines[*at] == OPEN).collect();
    if opens.len() != 1 {
        return Err(format!(
            "{FILE} must open SERVED_TURN_REFUSALS once, as `{OPEN}`; it does {} times",
            opens.len()
        ));
    }
    let mut rows: Vec<(String, String)> = Vec::new();
    let mut row: Option<(Option<String>, Option<String>)> = None;
    for line in &lines[opens[0] + 1..] {
        match *line {
            CLOSE => {
                if row.is_some() {
                    return Err(format!("{FILE}: SERVED_TURN_REFUSALS closes inside a row"));
                }
                if rows.is_empty() {
                    return Err(format!("{FILE}: SERVED_TURN_REFUSALS declares no refusal"));
                }
                for (at, (wire, recorded)) in rows.iter().enumerate() {
                    if rows[..at].iter().any(|(w, r)| w == wire || r == recorded) {
                        return Err(format!(
                            "{FILE}: SERVED_TURN_REFUSALS names {wire} or {recorded} twice"
                        ));
                    }
                }
                return Ok(rows);
            }
            "  {" => {
                if row.replace((None, None)).is_some() {
                    return Err(format!(
                        "{FILE}: SERVED_TURN_REFUSALS opens a row inside a row"
                    ));
                }
            }
            "  }," => {
                let Some((Some(wire), Some(recorded))) = row.take() else {
                    return Err(format!(
                        "{FILE}: a row of SERVED_TURN_REFUSALS lacks its wireType or its recordedName"
                    ));
                };
                rows.push((wire, recorded));
            }
            _ => {
                let Some((wire, recorded)) = row.as_mut() else {
                    return Err(format!(
                        "{FILE}: SERVED_TURN_REFUSALS holds a line outside its rows: {line:?}"
                    ));
                };
                for (key, slot) in [("    wireType: ", wire), ("    recordedName: ", recorded)] {
                    if let Some(value) = line.strip_prefix(key) {
                        let name = value
                            .strip_prefix('\'')
                            .and_then(|value| value.strip_suffix("',"))
                            .filter(|name| {
                                !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric())
                            })
                            .ok_or_else(|| {
                                format!("{FILE}: SERVED_TURN_REFUSALS declares {line:?}, not one quoted name")
                            })?;
                        if slot.replace(name.to_string()).is_some() {
                            return Err(format!(
                                "{FILE}: a row of SERVED_TURN_REFUSALS declares {} twice",
                                key.trim()
                            ));
                        }
                    }
                }
            }
        }
    }
    Err(format!(
        "{FILE}: SERVED_TURN_REFUSALS is never closed with `{CLOSE}`"
    ))
}

#[cfg(test)]
mod tests {
    use super::read;

    /// A patch that writes the declaring file with `rows` as its table, beside
    /// another file.
    fn patch(rows: &str) -> String {
        let file = format!(
            "/** The refusals. */\nexport const SERVED_TURN_REFUSALS = [\n{rows}] as const;\n\nexport type Kind = number;\n"
        );
        let added: String = file.lines().map(|line| format!("+{line}\n")).collect();
        format!(
            "diff --git a/packages/a.ts b/packages/a.ts\n--- a/packages/a.ts\n+++ b/packages/a.ts\n@@ -1 +1 @@\n-a\n+b\n\
             diff --git a/packages/core/src/core/generation-refusal.ts b/packages/core/src/core/generation-refusal.ts\n\
             new file mode 100644\n--- /dev/null\n+++ b/packages/core/src/core/generation-refusal.ts\n@@ -0,0 +1,{} @@\n{added}",
            file.lines().count()
        )
    }

    const ROW: &str = "  {\n    wireType: 'AError',\n    cause: 'a',\n    recordedName: 'ARefusal',\n    notice: refusedTurnNotice(\n      'why',\n      \"what's next\",\n    ),\n  },\n";

    #[test]
    fn reads_each_rows_wire_type_and_recorded_name_in_declared_order() {
        let second = ROW
            .replace("AError", "BError")
            .replace("ARefusal", "BRefusal");
        assert_eq!(
            read(&patch(&format!("{ROW}{second}"))).unwrap(),
            [
                ("AError".to_string(), "ARefusal".to_string()),
                ("BError".to_string(), "BRefusal".to_string()),
            ]
        );
    }

    #[test]
    fn refuses_every_other_shape_naming_what_it_found() {
        let cases = [
            (
                "no table",
                patch("").replace(
                    "export const SERVED_TURN_REFUSALS = [",
                    "export const OTHER = [",
                ),
                "must open SERVED_TURN_REFUSALS once",
            ),
            ("no row", patch(""), "declares no refusal"),
            (
                "no recorded name",
                patch(&ROW.replace("    recordedName: 'ARefusal',\n", "")),
                "lacks its wireType or its recordedName",
            ),
            (
                "a name twice",
                patch(&format!("{ROW}{}", ROW.replace("ARefusal", "BRefusal"))),
                "names AError or BRefusal twice",
            ),
            (
                "a computed name",
                patch(&ROW.replace("'AError',", "`${A}`,")),
                "not one quoted name",
            ),
            (
                "a key twice",
                patch(&ROW.replace("    cause: 'a',", "    wireType: 'CError',")),
                "declares wireType: twice",
            ),
            (
                "a line between rows",
                patch(&format!("{ROW}  ...more,\n")),
                "outside its rows",
            ),
            (
                "unclosed",
                {
                    let whole = patch(ROW);
                    whole[..whole.find("+] as const;").unwrap()].to_string()
                },
                "never closed",
            ),
            (
                "the file twice",
                {
                    let once = patch(ROW);
                    let at = once.find("diff --git a/packages/core").unwrap();
                    format!("{once}{}", &once[at..])
                },
                "writes packages/core/src/core/generation-refusal.ts 2 times",
            ),
        ];
        for (name, patch, cause) in cases {
            let refused = read(&patch).expect_err(name);
            assert!(refused.contains(cause), "{name}: {refused}");
        }
    }
}
