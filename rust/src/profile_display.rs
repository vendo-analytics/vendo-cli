//! Profile listing and switching (port of `src/profile-display.ts`).

use anyhow::{Result, bail};

use crate::{
    config::{DEFAULT_BASE_URL, EffectiveConfig, ProfileSummary},
    context::Ctx,
    output::{SelectOption, bold, dim, green, print_success, search_select_option},
};

pub fn format_profile_label(profile: &ProfileSummary, annotate_active: bool) -> String {
    let mut parts = vec![
        if annotate_active && profile.active { format!("{} (active)", profile.name) } else { profile.name.clone() },
        profile.account_id.clone().unwrap_or_else(|| "no account".to_string()),
    ];
    if profile.base_url != DEFAULT_BASE_URL {
        parts.push(profile.base_url.clone());
    }
    parts.join("  ")
}

pub fn format_profile_list_line(profile: &ProfileSummary, annotate_active: bool, indent: &str) -> String {
    let marker = if profile.active { green("*") } else { " ".to_string() };
    format!("{indent}{marker} {}", format_profile_label(profile, annotate_active))
}

/// Prints the list, or `empty_message` when there are none. `true` if any.
pub fn print_profile_list(
    profiles: &[ProfileSummary],
    annotate_active: bool,
    indent: &str,
    empty_message: &str,
) -> bool {
    if profiles.is_empty() {
        println!("{}", dim(empty_message));
        return false;
    }
    for profile in profiles {
        println!("{}", format_profile_list_line(profile, annotate_active, indent));
    }
    true
}

pub fn print_current_profile_summary(config: &EffectiveConfig, current: Option<&ProfileSummary>) {
    println!("{}", bold("Current Profile"));
    println!();
    println!("  Profile:     {}", config.selected_profile.clone().unwrap_or_else(|| dim("none selected")));
    println!("  Account ID:  {}", config.account_id.clone().unwrap_or_else(|| dim("not set")));
    println!("  Base URL:    {}", config.base_url);

    let overrides = config.env_override_names();
    if !overrides.is_empty() {
        println!();
        println!("{}", dim(&format!("  Env overrides active: {}", overrides.join(", "))));
    }

    if let Some(profile) = current {
        println!();
        println!("{}", dim(&format!("  Saved profile: {}", format_profile_label(profile, true))));
    }
}

pub struct SwitchOptions<'a> {
    pub profile_name: Option<String>,
    pub account_id: Option<String>,
    pub empty_message: &'a str,
    pub list_command: &'a str,
    pub profile_command: &'a str,
    pub verify_hint: &'a str,
}

pub fn switch_profile_selection(ctx: &Ctx, profiles: &[ProfileSummary], opts: SwitchOptions) -> Result<()> {
    if profiles.is_empty() {
        println!("{}", dim(opts.empty_message));
        return Ok(());
    }
    let mut profile_name = opts.profile_name;
    if profile_name.is_some() && opts.account_id.is_some() {
        bail!("Choose either a profile name or `--account <accountId>`, not both.");
    }

    if let Some(account_id) = &opts.account_id {
        let matches = ctx.store.find_profiles_by_account_id(account_id);
        match matches.as_slice() {
            [] => bail!(
                "No profile found for account ID \"{account_id}\".\n{}",
                dim(&format!("  Run `{}` to inspect configured profiles.", opts.list_command))
            ),
            [only] => profile_name = Some(only.name.clone()),
            many => bail!(
                "Multiple profiles use account ID \"{account_id}\".\n{}\n{}",
                many.iter().map(|m| dim(&format!("  {}", m.name))).collect::<Vec<_>>().join("\n"),
                dim(&format!("  Switch by profile name with `{} <profile>`.", opts.profile_command))
            ),
        }
    }

    if profile_name.is_none() {
        let options: Vec<SelectOption> = profiles
            .iter()
            .map(|p| SelectOption {
                value: p.name.clone(),
                label: format_profile_label(p, true),
                search_text: format!("{} {} {}", p.name, p.account_id.clone().unwrap_or_default(), p.base_url),
            })
            .collect();
        profile_name = search_select_option("Search and select a profile", &options);
    }

    let Some(profile_name) = profile_name else {
        println!("{}", dim("Cancelled."));
        return Ok(());
    };
    let Some(target) = profiles.iter().find(|p| p.name == profile_name) else {
        bail!(
            "Profile \"{profile_name}\" not found.\n{}",
            dim(&format!("  Run `{}` to inspect configured profiles.", opts.list_command))
        );
    };

    ctx.store.set_active_profile(&target.name)?;
    print_success(&format!("Switched to profile {}.", bold(&target.name)));
    println!(
        "{}",
        dim(&format!(
            "  Account ID: {}  Base URL: {}",
            target.account_id.as_deref().unwrap_or("not set"),
            target.base_url
        ))
    );
    println!("{}", dim(&format!("  Verify with {}.", opts.verify_hint)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, active: bool, account: Option<&str>, base: &str) -> ProfileSummary {
        ProfileSummary { name: name.into(), active, account_id: account.map(Into::into), base_url: base.into() }
    }

    #[test]
    fn labels_show_account_and_non_default_base_url() {
        assert_eq!(format_profile_label(&profile("a", true, Some("acct"), DEFAULT_BASE_URL), true), "a (active)  acct");
        assert_eq!(format_profile_label(&profile("a", true, Some("acct"), DEFAULT_BASE_URL), false), "a  acct");
        assert_eq!(
            format_profile_label(&profile("b", false, None, "https://stg.vendodata.com"), true),
            "b  no account  https://stg.vendodata.com"
        );
    }

    #[test]
    fn list_lines_mark_the_active_profile() {
        assert_eq!(
            format_profile_list_line(&profile("a", true, Some("x"), DEFAULT_BASE_URL), false, "    "),
            "    * a  x"
        );
        assert_eq!(format_profile_list_line(&profile("b", false, Some("y"), DEFAULT_BASE_URL), false, ""), "  b  y");
    }
}
