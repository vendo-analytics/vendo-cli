//! Profile listing and switching (port of `src/profile-display.ts`).

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::{
    config::{DEFAULT_BASE_URL, ProfileSummary},
    context::Ctx,
    output::{bold, dim, green, print_json, print_success},
};

/// A profile as the `--json` of the profile commands prints it (VE-3831): what `profile list`
/// shows, `accountId` null where it says "no account". Never the API key.
pub fn profile_json(profile: &ProfileSummary) -> Value {
    json!({
        "name": profile.name,
        "active": profile.active,
        "accountId": profile.account_id,
        "baseUrl": profile.base_url,
    })
}

/// The base URL's host as the profile lists show it (the workspace screen's, VE-3891, and the list
/// `--profile` with no name opens, VE-3892): `stg.vendodata.com` for `https://stg.vendodata.com/`,
/// nothing for the default one, as `vendo profile list` leaves it out.
pub fn shown_host(profile: &ProfileSummary) -> &str {
    if profile.base_url == DEFAULT_BASE_URL {
        return "";
    }
    let url = profile.base_url.as_str();
    url.split_once("://").map_or(url, |(_, rest)| rest).trim_end_matches('/')
}

pub fn format_profile_label(profile: &ProfileSummary, annotate_active: bool) -> String {
    profile_label_parts(profile, annotate_active).join("  ")
}

/// The parts of a profile's label, which it joins: the name (`alpha (active)`), the account ID (`no
/// account` for none) and the base URL unless it is the default one.
fn profile_label_parts(profile: &ProfileSummary, annotate_active: bool) -> Vec<String> {
    let mut parts = vec![
        if annotate_active && profile.active { format!("{} (active)", profile.name) } else { profile.name.clone() },
        profile.account_id.clone().unwrap_or_else(|| "no account".to_string()),
    ];
    if profile.base_url != DEFAULT_BASE_URL {
        parts.push(profile.base_url.clone());
    }
    parts
}

/// `vendo profile list`'s line of `profile` as the cells of a selectable list's row (VE-3894): the
/// marker and the name (`* alpha (active)`, `  beta`), the account ID, and the base URL unless it is
/// the default one. Plain: the list pads them.
pub fn profile_list_cells(profile: &ProfileSummary) -> Vec<String> {
    let mut parts = profile_label_parts(profile, true);
    let marker = if profile.active { "*" } else { " " };
    parts[0] = format!("{marker} {}", parts[0]);
    parts
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

pub struct SwitchOptions<'a> {
    pub profile_name: Option<String>,
    pub account_id: Option<String>,
    pub empty_message: &'a str,
    pub list_command: &'a str,
    pub profile_command: &'a str,
    pub verify_hint: &'a str,
    /// `--json`: print `{ "profile": … }`, the profile now selected or null when nothing was
    /// switched, and never open the profile list.
    pub json: bool,
}

/// `{ "profile": null }`: nothing was switched.
fn print_no_switch(json: bool, message: &str) {
    if json {
        print_json(&json!({ "profile": null }));
    } else {
        println!("{}", dim(message));
    }
}

/// The profile chosen from `profiles` in the arrow-key list titled `title` (VE-3892, decided by Yalcin
/// 2026-10-07: it replaced the numbered picker), where the group menu opens
/// ([`crate::output::can_show_menu`]). `None` elsewhere: nothing is switched, as without a terminal.
#[cfg(feature = "menu")]
fn choose_profile(title: &str, profiles: &[ProfileSummary]) -> Option<String> {
    crate::ask::choose_profile(title, profiles)
}

/// A build without the `menu` feature has no list: nothing is chosen, as without a terminal.
#[cfg(not(feature = "menu"))]
fn choose_profile(_title: &str, _profiles: &[ProfileSummary]) -> Option<String> {
    None
}

pub fn switch_profile_selection(ctx: &Ctx, profiles: &[ProfileSummary], opts: SwitchOptions) -> Result<()> {
    if profiles.is_empty() {
        print_no_switch(opts.json, opts.empty_message);
        return Ok(());
    }
    // Empty values count as unset, as the TS CLI's truthiness checks had it.
    let mut profile_name = opts.profile_name.filter(|name| !name.is_empty());
    let account_id = opts.account_id.filter(|id| !id.is_empty());
    if profile_name.is_some() && account_id.is_some() {
        bail!("Choose either a profile name or `--account <accountId>`, not both.");
    }

    if let Some(account_id) = &account_id {
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

    if profile_name.is_none() && !opts.json {
        // The profile list `--profile` with no name opens, titled with the command (VE-3892).
        profile_name = choose_profile(opts.profile_command, profiles);
    }

    let Some(profile_name) = profile_name else {
        print_no_switch(opts.json, "Cancelled.");
        return Ok(());
    };
    let Some(target) = profiles.iter().find(|p| p.name == profile_name) else {
        bail!(
            "Profile \"{profile_name}\" not found.\n{}",
            dim(&format!("  Run `{}` to inspect configured profiles.", opts.list_command))
        );
    };

    ctx.store.set_active_profile(&target.name)?;
    if opts.json {
        // Read back: `active` says whether it is now the one commands use (--profile or
        // VENDO_PROFILE may still select another).
        let switched = ctx.store.profile_summaries().into_iter().find(|p| p.name == target.name);
        print_json(&json!({ "profile": switched.as_ref().map(profile_json) }));
        return Ok(());
    }
    print_success(&format!("Switched to profile {}.", bold(&target.name)));
    println!(
        "{}",
        dim(&format!(
            "  Account ID: {}  Base URL: {}",
            target.account_id.as_deref().unwrap_or("not set"),
            target.base_url
        ))
    );
    // An explicit switch changes the saved active profile, which VENDO_PROFILE still overrides in
    // this shell (Yalcin, 2026-10-06).
    if let Some(name) = ctx.store.vendo_profile().filter(|name| *name != target.name) {
        println!(
            "{}",
            dim(&format!(
                "  VENDO_PROFILE={name} still overrides it in this shell: unset VENDO_PROFILE to use {} here.",
                target.name
            ))
        );
    }
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
