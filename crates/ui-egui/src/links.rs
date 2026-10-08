//! Product and support links (Help menu, About dialog, Home screen).
//!
//! JBD is made by Jabrod: its page, downloads, Pro and support live on jabrod.com. The open-source
//! software JBD includes is credited in THIRD-PARTY-NOTICES.txt, shipped next to jbd.exe.

/// JBD's page on jabrod.com.
pub const APP_PAGE: &str = "https://jabrod.com/jbd";
/// The Jabrod website.
pub const WEBSITE: &str = "https://jabrod.com";
/// Support: an email to the Jabrod team.
pub const SUPPORT: &str = "mailto:contact@jabrod.com?subject=JBD%20support";
/// Jabrod's terms of service and privacy policy.
pub const TERMS: &str = "https://jabrod.com/terms";

/// (command id, label, url) for every link, in menu order.
pub const ALL: [(&str, &str, &str); 4] = [
    ("help.appPage", "JBD on jabrod.com", APP_PAGE),
    ("help.support", "Contact Support…", SUPPORT),
    ("help.website", "Jabrod Website", WEBSITE),
    ("help.terms", "Terms and Privacy", TERMS),
];

/// The URL a `help.*` link command opens.
pub fn url_for(command: &str) -> Option<&'static str> {
    ALL.iter().find(|(id, _, _)| *id == command).map(|(_, _, u)| *u)
}

/// Open `url` in the system browser (a new tab on the web).
pub fn open(ctx: &egui::Context, url: &str) {
    ctx.open_url(egui::OpenUrl::new_tab(url));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_link_is_a_help_command_with_a_url() {
        assert!(ALL.iter().all(|(id, _, u)| id.starts_with("help.") && (u.starts_with("https://") || u.starts_with("mailto:"))));
        assert_eq!(url_for("help.appPage"), Some(APP_PAGE));
        assert_eq!(url_for("help.support"), Some(SUPPORT));
        assert_eq!(url_for("help.nope"), None);
    }

    /// Nothing in JBD may point at the ArtCraft community: its marks are not licensed to forks.
    #[test]
    fn no_artcraft_links() {
        assert!(ALL.iter().all(|(_, label, u)| !label.contains("ArtCraft") && !u.contains("artcraft")));
        // and none of them name FilmCraft (it is credited in THIRD-PARTY-NOTICES.txt)
        assert!(ALL.iter().all(|(_, label, u)| !label.contains("FilmCraft") && !u.contains("filmcraft")));
    }
}
