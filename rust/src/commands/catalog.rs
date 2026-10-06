//! `vendo catalog …` (port of `src/commands/catalog.ts`).

use anyhow::Result;
use serde_json::{Value, json};

use crate::{
    client::payload,
    context::Ctx,
    jobs::Job,
    output::{
        OutputMode, bold, cyan, dim, green, js_if, js_join, js_template, js_truthy, print_count, print_field,
        print_json, resolve_output_mode, run_action, table,
    },
};

/// `item.supportedRoles.join(', ')`: the Roles column, also of the list `apps create` shows when it is
/// missing its `--type` (VE-3881).
pub(crate) fn roles(item: &Value) -> String {
    item.get("supportedRoles").and_then(Value::as_array).map(|r| js_join(r, ", ")).unwrap_or_default()
}

/// `item.selfServe ? 'yes' : 'no'`.
fn self_serve(item: &Value) -> String {
    if item.get("selfServe").is_some_and(js_truthy) { green("yes") } else { dim("no") }
}

/// The API's own `availability` in plain words (VE-3829), as the list's Availability column and
/// `catalog get` show it. The routes send `self_serve` or `request_access` (they drop `hidden`
/// entries); any other value prints as sent. `None` when the API sends none (before VE-2436).
fn availability_words(item: &Value) -> Option<String> {
    let availability = Job(item).text("availability")?;
    Some(match availability.as_str() {
        "self_serve" => green("ready"),
        "request_access" => dim("on request"),
        _ => availability,
    })
}

/// The Availability column: empty when the API sends no `availability`, with no guess from `selfServe`.
fn availability(item: &Value) -> String {
    availability_words(item).unwrap_or_default()
}

/// `35 ready · 560 more on request (vendo catalog list --all)`, from the counts in the response's
/// `meta`. The API counts after `--category` and `--role`, so the footer does too, and its hint
/// repeats them, `--category` first and the values as given (Yalcin, 2026-10-06), so it lists the
/// platforms counted. An empty value filters nothing at the API, so the hint leaves it out. `None`
/// when the API sends no counts (before VE-2436): the plain count line stays.
fn ready_footer(res: &Value, category: Option<&str>, role: Option<&str>) -> Option<String> {
    let count = |key: &str| res.get("meta")?.get(key)?.as_u64();
    let (ready, on_request) = (count("selfServeTotal")?, count("requestAccessTotal")?);
    if on_request == 0 {
        return Some(format!("{ready} ready"));
    }
    let filters: String = [("--category", category), ("--role", role)]
        .into_iter()
        .filter_map(|(flag, value)| value.filter(|v| !v.is_empty()).map(|v| format!(" {flag} {v}")))
        .collect();
    Some(format!("{ready} ready · {on_request} more on request (vendo catalog list{filters} --all)"))
}

pub async fn list(
    ctx: &Ctx,
    category: Option<String>,
    role: Option<String>,
    all: bool,
    json: bool,
    output: Option<String>,
) -> Result<()> {
    let client = ctx.client()?;
    // The route returns every entry at once (no pagination); without the flag it leaves out the
    // request-access ones.
    let query = [
        ("category", category.clone()),
        ("role", role.clone()),
        ("include_request_access", all.then(|| "true".to_string())),
    ];
    let res = run_action("Fetching catalog...", client.get("/catalog", &query)).await?;
    let rows = payload(&res).as_array().cloned().unwrap_or_default();
    match resolve_output_mode(json, output.as_deref()) {
        OutputMode::Json => print_json(&res),
        OutputMode::Field => print_field(&rows, output.as_deref().unwrap_or_default()),
        OutputMode::Table => {
            let mut grid = table(&["App Type", "Name", "Category", "Roles", "Availability"]);
            for item in &rows {
                let t = |key: &str| Job(item).text(key).unwrap_or_default();
                grid.add_row(vec![
                    cyan(&t("appType")),
                    t("displayName"),
                    t("category"),
                    roles(item),
                    availability(item),
                ]);
            }
            println!("{grid}");
            // `--all` shows every entry, so it keeps the plain count line.
            match if all { None } else { ready_footer(&res, category.as_deref(), role.as_deref()) } {
                Some(footer) => println!("{}", dim(&footer)),
                None => print_count(rows.len() as u64, "platform"),
            }
        }
    }
    Ok(())
}

pub async fn get(ctx: &Ctx, app_type: &str, json: bool) -> Result<()> {
    let client = ctx.client()?;
    let res = run_action("Fetching catalog entry...", client.get(&format!("/catalog/{app_type}"), &[])).await?;
    if json {
        print_json(&res);
        return Ok(());
    }
    for line in catalog_lines(payload(&res)) {
        println!("{line}");
    }
    Ok(())
}

/// The `catalog get` view, with the TS CLI's `${…}` rendering and truthiness checks.
fn catalog_lines(item: &Value) -> Vec<String> {
    let field = |key: &str| item.get(key);
    // The list's words for the API's `availability` (Yalcin, 2026-10-06); an API that sends none
    // (before VE-2436) keeps the Self-Serve line.
    let (availability_label, availability_value) = match availability_words(item) {
        Some(words) => ("Availability:", words),
        None => ("Self-Serve:", self_serve(item)),
    };
    // Values line up two spaces after the longest label, as they did after `Self-Serve:`.
    let width = availability_label.len() + 2;
    let row = |label: &str, value: &str| format!("  {label:<width$}{value}");
    let mut lines = vec![
        String::new(),
        format!(
            "{} {}",
            bold(&js_template(field("displayName"))),
            dim(&format!("({})", js_template(field("appType"))))
        ),
        String::new(),
        row("Category:", &js_template(field("category"))),
        row("Roles:", &roles(item)),
        row(availability_label, &availability_value),
    ];
    if let Some(lifecycle) = js_if(field("lifecycle")) {
        lines.push(row("Lifecycle:", &lifecycle));
    }
    if let Some(provider) = js_if(field("provider")) {
        lines.push(row("Provider:", &provider));
    }
    if let Some(description) = js_if(field("description")) {
        lines.extend([String::new(), format!("  {description}")]);
    }
    if let Some(docs) = js_if(field("documentationUrl")) {
        lines.extend([String::new(), format!("  Docs: {}", cyan(&docs))]);
    }
    let fields = credential_fields(item);
    if !fields.is_empty() {
        lines.extend([String::new(), bold("  Credential Fields:")]);
        for field in &fields {
            lines.extend(credential_field_lines(field, "    ", "      "));
        }
    }
    lines
}

fn credential_fields(item: &Value) -> Vec<Value> {
    item.get("credentialFields").and_then(Value::as_array).cloned().unwrap_or_default()
}

/// `${field.label} (${field.name}) — ${field.type}`, then the description when truthy.
fn credential_field_lines(field: &Value, indent: &str, description_indent: &str) -> Vec<String> {
    let t = |key: &str| js_template(field.get(key));
    let mut lines = vec![format!("{indent}{} ({}) — {}", t("label"), dim(&t("name")), t("type"))];
    if let Some(description) = js_if(field.get("description")) {
        lines.push(format!("{description_indent}{}", dim(&description)));
    }
    lines
}

pub async fn credential_schema(ctx: &Ctx, app_type: &str, json: bool) -> Result<()> {
    let client = ctx.client()?;
    let res = run_action("Fetching credential schema...", client.get(&format!("/catalog/{app_type}"), &[])).await?;
    if json {
        // `res.data?.credentialFields ?? []`, as the API sent it.
        let fields = payload(&res).get("credentialFields").filter(|v| !v.is_null()).cloned().unwrap_or(json!([]));
        print_json(&json!({ "appType": app_type, "credentialFields": fields }));
        return Ok(());
    }
    let fields = credential_fields(payload(&res));
    if fields.is_empty() {
        println!("{}", dim(&format!("No credential fields declared for {app_type}.")));
        return Ok(());
    }
    println!("{}", bold(&format!("Credential fields for {app_type}:")));
    for field in &fields {
        for line in credential_field_lines(field, "  ", "    ") {
            println!("{line}");
        }
    }
    println!();
    println!("{}", dim("Use these as keys in the JSON file you pass to --credentials-file for `vendo apps create`."));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_shows_the_availability_the_list_shows_in_place_of_self_serve() {
        // Yalcin, 2026-10-06 (VE-3829): the list's words from the API's `availability`, not `selfServe`;
        // the labels line up two spaces after the longest one, as they did after `Self-Serve:`.
        let item = |availability: Value| {
            json!({ "appType": "tiktok_ads", "displayName": "TikTok Ads", "category": "advertising",
                    "supportedRoles": ["source"], "selfServe": true, "availability": availability,
                    "lifecycle": "ga", "provider": "tiktok" })
        };
        for (availability, words) in
            [(json!("self_serve"), "ready"), (json!("request_access"), "on request"), (json!("beta"), "beta")]
        {
            assert_eq!(
                catalog_lines(&item(availability.clone())),
                [
                    "",
                    "TikTok Ads (tiktok_ads)",
                    "",
                    "  Category:      advertising",
                    "  Roles:         source",
                    &format!("  Availability:  {words}"),
                    "  Lifecycle:     ga",
                    "  Provider:      tiktok",
                ],
                "{availability}"
            );
        }
        // No availability (an API from before VE-2436): the view as it was, Self-Serve line and all.
        assert_eq!(
            catalog_lines(&item(Value::Null))[3..6],
            ["  Category:    advertising", "  Roles:       source", "  Self-Serve:  yes"]
        );
    }

    #[test]
    fn get_prints_missing_null_and_falsy_fields_like_the_ts_cli() {
        // Expected lines from the TS CLI 0.3.1 run against a local stub with the same body (VE-3728),
        // which has no `availability`: an API from before VE-2436 keeps the Self-Serve line.
        let item = json!({
            "appType": "x", "displayName": null, "supportedRoles": ["source", null], "selfServe": "yes", "lifecycle": 0,
            "provider": "", "description": "d", "documentationUrl": null,
            "credentialFields": [{ "name": null, "label": "L", "type": 5, "description": 0 }, { "label": "M" }],
        });
        assert_eq!(
            catalog_lines(&item),
            [
                "",
                "null (x)",
                "",
                "  Category:    undefined",
                "  Roles:       source, ",
                "  Self-Serve:  yes",
                "",
                "  d",
                "",
                "  Credential Fields:",
                "    L (null) — 5",
                "    M (undefined) — undefined",
            ]
        );
    }

    #[test]
    fn self_serve_is_javascript_truthiness() {
        for (value, want) in [
            (json!(true), "yes"),
            (json!(1), "yes"),
            (json!("no"), "yes"),
            (json!(0), "no"),
            (json!(""), "no"),
            (json!(null), "no"),
        ] {
            assert_eq!(self_serve(&json!({ "selfServe": value })), want, "{value}");
        }
        assert_eq!(self_serve(&json!({})), "no");
    }

    #[test]
    fn availability_is_the_api_value_in_plain_words() {
        for (value, want) in [
            (json!("self_serve"), "ready"),
            (json!("request_access"), "on request"),
            // Not sent today (the route drops `hidden`): shown as the API wrote it.
            (json!("hidden"), "hidden"),
            (json!("beta"), "beta"),
            (json!(null), ""),
        ] {
            assert_eq!(availability(&json!({ "availability": value, "selfServe": true })), want, "{value}");
        }
        assert_eq!(availability(&json!({ "selfServe": true })), "", "no guess from selfServe");
    }

    #[test]
    fn the_footer_hint_repeats_the_filters() {
        // Yalcin, 2026-10-06: the hint lists the platforms the counts counted, so it keeps `--category`
        // and `--role`, in that order, with the values as given.
        let res = json!({ "data": [], "meta": { "total": 9, "selfServeTotal": 9, "requestAccessTotal": 5 } });
        let hint = |category: Option<&str>, role: Option<&str>| ready_footer(&res, category, role);
        let on_request = |command: &str| Some(format!("9 ready · 5 more on request ({command})"));
        assert_eq!(hint(Some("advertising"), None), on_request("vendo catalog list --category advertising --all"));
        assert_eq!(hint(None, Some("source")), on_request("vendo catalog list --role source --all"));
        assert_eq!(
            hint(Some("crm"), Some("destination")),
            on_request("vendo catalog list --category crm --role destination --all")
        );
        assert_eq!(
            hint(Some("Ads"), Some("Source")),
            on_request("vendo catalog list --category Ads --role Source --all")
        );
        assert_eq!(hint(None, None), on_request("vendo catalog list --all"), "no filters: the text as decided");
        // An empty value filters nothing at the API (`if (categoryFilter)`), so the hint leaves it out
        // rather than print a command that does not parse.
        assert_eq!(hint(Some(""), Some("source")), on_request("vendo catalog list --role source --all"));
        // Nothing on request: no hint to repeat them in.
        let none = json!({ "data": [], "meta": { "total": 2, "selfServeTotal": 2, "requestAccessTotal": 0 } });
        assert_eq!(ready_footer(&none, Some("crm"), Some("source")).as_deref(), Some("2 ready"));
    }

    #[test]
    fn the_footer_takes_its_counts_from_meta() {
        let footer = |meta: Value| ready_footer(&json!({ "data": [], "meta": meta }), None, None);
        assert_eq!(
            footer(json!({ "total": 35, "selfServeTotal": 35, "requestAccessTotal": 560 })).as_deref(),
            Some("35 ready · 560 more on request (vendo catalog list --all)")
        );
        assert_eq!(
            footer(json!({ "total": 2, "selfServeTotal": 2, "requestAccessTotal": 0 })).as_deref(),
            Some("2 ready")
        );
        assert_eq!(
            footer(json!({ "total": 0, "selfServeTotal": 0, "requestAccessTotal": 4 })).as_deref(),
            Some("0 ready · 4 more on request (vendo catalog list --all)")
        );
        // An API without the counts (before VE-2436), or with only one of them: the plain count line.
        assert_eq!(ready_footer(&json!({ "data": [] }), None, None), None);
        assert_eq!(footer(json!({ "total": 3 })), None);
        assert_eq!(footer(json!({ "selfServeTotal": 3 })), None);
        assert_eq!(footer(json!({ "selfServeTotal": 3, "requestAccessTotal": "1" })), None);
    }
}
