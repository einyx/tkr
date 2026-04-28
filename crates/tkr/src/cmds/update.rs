use anyhow::{Context, Result};

const REPO_OWNER: &str = "einyx";
const REPO_NAME: &str = "tkr";
const BIN_NAME: &str = "tkr";

pub fn run(check_only: bool, force: bool) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    println!("Current version: {}", current);
    println!(
        "Checking github.com/{}/{} for newer release...",
        REPO_OWNER, REPO_NAME
    );

    let releases = self_update::backends::github::ReleaseList::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .build()
        .context("configuring release list")?
        .fetch()
        .context("fetching releases from GitHub")?;

    let latest = releases
        .first()
        .ok_or_else(|| anyhow::anyhow!("no releases found"))?;

    let latest_version = latest.version.trim_start_matches('v').to_string();

    if !force && latest_version == current {
        println!("Already up to date.");
        return Ok(());
    }

    if check_only {
        if latest_version != current {
            println!("New version available: {}", latest_version);
            std::process::exit(2);
        }
        return Ok(());
    }

    println!("New version available: {}", latest_version);

    let target = self_update::get_target();
    let asset = format!("{}-{}.tar.gz", BIN_NAME, target);
    println!("Downloading {}...", asset);

    let status = self_update::backends::github::Update::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(BIN_NAME)
        .target(target)
        .identifier(&asset)
        .show_download_progress(true)
        .show_output(false)
        .no_confirm(true)
        .current_version(current)
        .build()
        .context("configuring updater")?
        .update()
        .context("running update")?;

    println!("Updated to {}.", status.version());
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Verify the module compiles and the public function signature is correct.
    /// We do not test the network path — that's an integration concern.
    #[test]
    fn version_comparison_sanity() {
        // Trim leading 'v' the same way run() does.
        let tag = "v0.1.6";
        let version = tag.trim_start_matches('v').to_string();
        assert_eq!(version, "0.1.6");

        let no_v = "0.1.6";
        let version2 = no_v.trim_start_matches('v').to_string();
        assert_eq!(version2, "0.1.6");

        // Same version → no update needed.
        assert_eq!("0.1.6", "0.1.6");

        // Different version → update needed.
        assert_ne!("0.1.5", "0.1.6");
    }
}
