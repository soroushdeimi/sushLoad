//! Site folders: which web site a download came from, which group that site belongs to
//! (University, Software, …) and where the file goes because of it. Pure functions and
//! plain data only; the engine owns the state and the optional on-device AI (`sites_ai`).

use crate::categories;
use crate::model::{Category, Settings, SiteFolders};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::PathBuf;

/// The kinds of site a download can be filed under (`SiteFolders::GroupAndSite`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteGroup {
    University,
    Government,
    Software,
    CloudStorage,
    Video,
    Music,
    News,
    Shopping,
    Social,
    BooksPapers,
    Other,
}

impl SiteGroup {
    pub const ALL: [SiteGroup; 11] = [
        SiteGroup::University,
        SiteGroup::Government,
        SiteGroup::Software,
        SiteGroup::CloudStorage,
        SiteGroup::Video,
        SiteGroup::Music,
        SiteGroup::News,
        SiteGroup::Shopping,
        SiteGroup::Social,
        SiteGroup::BooksPapers,
        SiteGroup::Other,
    ];

    /// Folder name.
    pub fn label(self) -> &'static str {
        match self {
            SiteGroup::University => "University",
            SiteGroup::Government => "Government",
            SiteGroup::Software => "Software",
            SiteGroup::CloudStorage => "Cloud storage",
            SiteGroup::Video => "Video",
            SiteGroup::Music => "Music",
            SiteGroup::News => "News",
            SiteGroup::Shopping => "Shopping",
            SiteGroup::Social => "Social",
            SiteGroup::BooksPapers => "Books & papers",
            SiteGroup::Other => "Other",
        }
    }

    /// Stable id used in the JSON API.
    pub fn id(self) -> &'static str {
        match self {
            SiteGroup::University => "university",
            SiteGroup::Government => "government",
            SiteGroup::Software => "software",
            SiteGroup::CloudStorage => "cloud_storage",
            SiteGroup::Video => "video",
            SiteGroup::Music => "music",
            SiteGroup::News => "news",
            SiteGroup::Shopping => "shopping",
            SiteGroup::Social => "social",
            SiteGroup::BooksPapers => "books_papers",
            SiteGroup::Other => "other",
        }
    }

    pub fn from_id(id: &str) -> Option<SiteGroup> {
        SiteGroup::ALL.into_iter().find(|g| g.id() == id)
    }

    pub fn from_label(label: &str) -> Option<SiteGroup> {
        SiteGroup::ALL
            .into_iter()
            .find(|g| g.label().eq_ignore_ascii_case(label.trim()))
    }

    /// Short description handed to the on-device model next to the group name.
    pub fn hint(self) -> &'static str {
        match self {
            SiteGroup::University => {
                "a university, college or school: courses, lectures, academic institution"
            }
            SiteGroup::Government => {
                "a government body, ministry or public authority: official documents and forms"
            }
            SiteGroup::Software => {
                "software, developer tools, programming, app installers, open source projects"
            }
            SiteGroup::CloudStorage => "file sharing and cloud storage services",
            SiteGroup::Video => "video streaming, films and TV",
            SiteGroup::Music => "music, podcasts and audio streaming",
            SiteGroup::News => "news outlets, newspapers and magazines",
            SiteGroup::Shopping => "online shops and marketplaces",
            SiteGroup::Social => "social networks and messaging",
            SiteGroup::BooksPapers => {
                "books, e-books, research papers, journals, libraries and online learning material"
            }
            SiteGroup::Other => "anything else",
        }
    }
}

/// One row of the Sites table in Settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteView {
    pub domain: String,
    /// Effective group.
    pub group: SiteGroup,
    /// `"auto"`, `"you"` or `"ai"`.
    pub origin: &'static str,
    /// Effective folder name.
    pub folder: String,
    pub user_group: Option<SiteGroup>,
    pub user_folder: Option<String>,
}

/// What is remembered per domain (in `state.json`). Everything is optional: a domain that was
/// merely seen has an all-empty entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SiteEntry {
    /// Group the user picked in Settings; wins over rules and AI.
    pub user_group: Option<SiteGroup>,
    /// Folder name the user typed; replaces the domain.
    pub user_folder: Option<String>,
    /// The on-device model has been asked about this domain (once, ever).
    pub ai_asked: bool,
    /// Its answer; None when it was unsure (or has not been asked).
    pub ai: Option<SiteGroup>,
}

/// Why a domain got its group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    User,
    Rule(String),
    Ai,
    Default,
}

impl Source {
    /// Badge shown in Settings.
    pub fn badge(&self) -> &'static str {
        match self {
            Source::User => "you",
            Source::Ai => "ai",
            Source::Rule(_) | Source::Default => "auto",
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Source::User => "user".to_string(),
            Source::Rule(r) => format!("rule {r}"),
            Source::Ai => "ai".to_string(),
            Source::Default => "default".to_string(),
        }
    }
}

// ---------- domain ----------

/// Two-label (and longer) public suffixes under which people register names. Anything not
/// listed is treated as a one-label TLD (`example.com`, `foo.dev`).
const MULTI_SUFFIXES: &[&str] = &[
    // Iran
    "ac.ir", "co.ir", "gov.ir", "org.ir", "net.ir", "sch.ir", "id.ir", // UK / Ireland
    "ac.uk", "co.uk", "gov.uk", "org.uk", "me.uk", "ltd.uk", "net.uk", "sch.uk", "nhs.uk",
    // Australia / NZ
    "edu.au", "com.au", "gov.au", "org.au", "net.au", "ac.nz", "co.nz", "govt.nz", "org.nz",
    // Japan / Korea / China / Taiwan / HK / SG / MY / ID / IN / PK / TH / VN
    "ac.jp", "co.jp", "go.jp", "or.jp", "ne.jp", "ac.kr", "co.kr", "go.kr", "or.kr", "com.cn",
    "edu.cn", "gov.cn", "org.cn", "net.cn", "com.tw", "edu.tw", "gov.tw", "org.tw", "com.hk",
    "edu.hk", "gov.hk", "com.sg", "edu.sg", "gov.sg", "com.my", "edu.my", "gov.my", "co.id",
    "ac.id", "go.id", "or.id", "co.in", "ac.in", "gov.in", "edu.in", "org.in", "net.in", "com.pk",
    "edu.pk", "gov.pk", "org.pk", "co.th", "ac.th", "go.th", "com.vn", "edu.vn", "gov.vn",
    // Turkey / Middle East / Africa
    "edu.tr", "com.tr", "gov.tr", "org.tr", "net.tr", "k12.tr", "com.sa", "edu.sa", "gov.sa",
    "org.sa", "com.eg", "edu.eg", "gov.eg", "org.eg", "ac.il", "co.il", "gov.il", "org.il",
    "ac.ae", "co.ae", "gov.ae", "com.qa", "edu.qa", "co.za", "ac.za", "gov.za", "org.za", "com.ng",
    "edu.ng", "gov.ng", "co.ke", "ac.ke", "go.ke", // Europe / Russia / Ukraine
    "com.ua", "edu.ua", "gov.ua", "org.ua", "com.pl", "edu.pl", "gov.pl", "com.ru", "org.ru",
    "gouv.fr", "com.es", "gob.es", "org.es", "edu.gr", "com.gr", "gov.gr", "com.pt", "gov.pt",
    "co.at", "or.at", "gv.at", // Americas
    "com.br", "edu.br", "gov.br", "org.br", "net.br", "com.ar", "edu.ar", "gob.ar", "gov.ar",
    "com.mx", "edu.mx", "gob.mx", "org.mx", "com.co", "edu.co", "gov.co", "com.pe", "edu.pe",
    "gob.pe", "com.ve", "edu.ve", "gob.ve", "com.ec", "edu.ec", "gob.ec", "co.cr", "ac.cr",
    "go.cr",
];

/// Hosts that are a site of their own although their registrable domain is far broader.
const KEEP_HOSTS: &[&str] = &[
    "drive.google.com",
    "docs.google.com",
    "drive.usercontent.google.com",
];

/// Lower-case host of an http(s) URL without port, or None.
pub fn host_of(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url.trim()).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed
        .host_str()?
        .trim_matches(['[', ']'])
        .trim_end_matches('.');
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

/// True for IP addresses and localhost names, which are never reduced or sent to the AI.
pub fn is_ip_or_local(host: &str) -> bool {
    host.parse::<IpAddr>().is_ok() || host == "localhost" || host.ends_with(".localhost")
}

/// Registrable domain of a host: `www.` dropped, `lecture.cs.sharif.edu` → `sharif.edu`,
/// `www.iust.ac.ir` → `iust.ac.ir`. IP addresses, localhost and single-label hosts stay as is;
/// IDN hosts are already ASCII (punycode) when they come from a parsed URL.
pub fn registrable_domain(host: &str) -> String {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if is_ip_or_local(&host) {
        return host;
    }
    for keep in KEEP_HOSTS {
        if host == *keep || host.ends_with(&format!(".{keep}")) {
            return (*keep).to_string();
        }
    }
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    if labels.len() <= 2 {
        return labels.join(".");
    }
    for i in 1..labels.len() {
        let suffix = labels[i..].join(".");
        if MULTI_SUFFIXES.contains(&suffix.as_str()) {
            return labels[i - 1..].join(".");
        }
    }
    labels[labels.len() - 2..].join(".")
}

/// The site a download belongs to: the Referer's host when the browser sent one (the page the
/// user was on, so CDN and storage hosts don't become sites), else the URL's host.
pub fn site_domain(url: &str, referer: Option<&str>) -> Option<String> {
    referer
        .and_then(host_of)
        .or_else(|| host_of(url))
        .map(|h| registrable_domain(&h))
        .filter(|d| !d.is_empty())
}

// ---------- groups ----------

/// Known sites, matched on domain boundaries (`github.com` matches `github.com` and
/// `x.github.com`, never `notgithub.com`). First hit wins.
const RULES: &[(&str, SiteGroup)] = &[
    // Software
    ("github.com", SiteGroup::Software),
    ("githubusercontent.com", SiteGroup::Software),
    ("gitlab.com", SiteGroup::Software),
    ("bitbucket.org", SiteGroup::Software),
    ("sourceforge.net", SiteGroup::Software),
    ("apple.com", SiteGroup::Software),
    ("jetbrains.com", SiteGroup::Software),
    ("microsoft.com", SiteGroup::Software),
    ("visualstudio.com", SiteGroup::Software),
    ("python.org", SiteGroup::Software),
    ("rust-lang.org", SiteGroup::Software),
    ("nodejs.org", SiteGroup::Software),
    ("npmjs.com", SiteGroup::Software),
    ("npmjs.org", SiteGroup::Software),
    ("brew.sh", SiteGroup::Software),
    ("pypi.org", SiteGroup::Software),
    ("crates.io", SiteGroup::Software),
    ("mozilla.org", SiteGroup::Software),
    // Cloud storage
    ("drive.google.com", SiteGroup::CloudStorage),
    ("docs.google.com", SiteGroup::CloudStorage),
    ("drive.usercontent.google.com", SiteGroup::CloudStorage),
    ("googleusercontent.com", SiteGroup::CloudStorage),
    ("dropbox.com", SiteGroup::CloudStorage),
    ("dropboxusercontent.com", SiteGroup::CloudStorage),
    ("mega.nz", SiteGroup::CloudStorage),
    ("mega.io", SiteGroup::CloudStorage),
    ("sharepoint.com", SiteGroup::CloudStorage),
    ("1drv.ms", SiteGroup::CloudStorage),
    ("icloud.com", SiteGroup::CloudStorage),
    ("box.com", SiteGroup::CloudStorage),
    ("wetransfer.com", SiteGroup::CloudStorage),
    ("we.tl", SiteGroup::CloudStorage),
    // Video
    ("youtube.com", SiteGroup::Video),
    ("youtu.be", SiteGroup::Video),
    ("googlevideo.com", SiteGroup::Video),
    ("vimeo.com", SiteGroup::Video),
    ("aparat.com", SiteGroup::Video),
    ("twitch.tv", SiteGroup::Video),
    ("dailymotion.com", SiteGroup::Video),
    ("netflix.com", SiteGroup::Video),
    // Music
    ("soundcloud.com", SiteGroup::Music),
    ("spotify.com", SiteGroup::Music),
    ("scdn.co", SiteGroup::Music),
    ("bandcamp.com", SiteGroup::Music),
    // Books & papers
    ("arxiv.org", SiteGroup::BooksPapers),
    ("researchgate.net", SiteGroup::BooksPapers),
    ("springer.com", SiteGroup::BooksPapers),
    ("sciencedirect.com", SiteGroup::BooksPapers),
    ("ieee.org", SiteGroup::BooksPapers),
    ("acm.org", SiteGroup::BooksPapers),
    ("libgen.is", SiteGroup::BooksPapers),
    ("libgen.rs", SiteGroup::BooksPapers),
    ("libgen.li", SiteGroup::BooksPapers),
    ("z-lib.org", SiteGroup::BooksPapers),
    ("z-library.sk", SiteGroup::BooksPapers),
    ("archive.org", SiteGroup::BooksPapers),
    ("jstor.org", SiteGroup::BooksPapers),
    ("wiley.com", SiteGroup::BooksPapers),
    ("nature.com", SiteGroup::BooksPapers),
    // Shopping
    ("amazon.com", SiteGroup::Shopping),
    ("amazon.co.uk", SiteGroup::Shopping),
    ("amazon.de", SiteGroup::Shopping),
    ("digikala.com", SiteGroup::Shopping),
    ("ebay.com", SiteGroup::Shopping),
    ("aliexpress.com", SiteGroup::Shopping),
    // Social
    ("twitter.com", SiteGroup::Social),
    ("x.com", SiteGroup::Social),
    ("twimg.com", SiteGroup::Social),
    ("instagram.com", SiteGroup::Social),
    ("cdninstagram.com", SiteGroup::Social),
    ("facebook.com", SiteGroup::Social),
    ("fbcdn.net", SiteGroup::Social),
    ("telegram.org", SiteGroup::Social),
    ("t.me", SiteGroup::Social),
    ("reddit.com", SiteGroup::Social),
    ("redd.it", SiteGroup::Social),
    ("linkedin.com", SiteGroup::Social),
    // News
    ("bbc.com", SiteGroup::News),
    ("bbc.co.uk", SiteGroup::News),
    ("cnn.com", SiteGroup::News),
    ("nytimes.com", SiteGroup::News),
    ("theguardian.com", SiteGroup::News),
    ("reuters.com", SiteGroup::News),
    ("washingtonpost.com", SiteGroup::News),
    ("bloomberg.com", SiteGroup::News),
    ("aljazeera.com", SiteGroup::News),
    ("irna.ir", SiteGroup::News),
    ("isna.ir", SiteGroup::News),
    ("tasnimnews.com", SiteGroup::News),
    ("farsnews.ir", SiteGroup::News),
];

fn matches_domain(domain: &str, pattern: &str) -> bool {
    domain == pattern
        || (domain.len() > pattern.len()
            && domain.ends_with(pattern)
            && domain.as_bytes()[domain.len() - pattern.len() - 1] == b'.')
}

/// Built-in decision for a domain (no user mapping, no AI): the group and the name of the rule
/// that matched, or None when the rules don't know it.
pub fn rule_group(domain: &str) -> Option<(SiteGroup, String)> {
    if is_ip_or_local(domain) {
        return None;
    }
    let labels: Vec<&str> = domain.split('.').collect();
    let n = labels.len();
    let last = labels[n - 1];
    let second = if n >= 3 { labels[n - 2] } else { "" };

    // Universities: .edu, .edu.xx, .ac.xx, uni-…, …university…
    if n >= 2 && last == "edu" {
        return Some((SiteGroup::University, ".edu".into()));
    }
    if second == "edu" {
        return Some((SiteGroup::University, ".edu.xx".into()));
    }
    if second == "ac" {
        return Some((SiteGroup::University, ".ac.xx".into()));
    }
    // Government: .gov, .mil, .gov.xx
    if n >= 2 && (last == "gov" || last == "mil") {
        return Some((SiteGroup::Government, format!(".{last}")));
    }
    if matches!(second, "gov" | "gouv" | "govt" | "gob") {
        return Some((SiteGroup::Government, ".gov.xx".into()));
    }
    for (pattern, group) in RULES {
        if matches_domain(domain, pattern) {
            return Some((*group, (*pattern).to_string()));
        }
    }
    // Name-based university hints on the registrable label (the part before the suffix).
    let name = if n >= 3 && MULTI_SUFFIXES.contains(&labels[n - 2..].join(".").as_str()) {
        labels[n - 3]
    } else if n >= 2 {
        labels[n - 2]
    } else {
        labels[0]
    };
    if name.starts_with("uni-")
        || name.starts_with("univ-")
        || name == "univ"
        || name.ends_with("-univ")
        || name.contains("university")
        || name.contains("universit")
        || name.contains("universidad")
    {
        return Some((SiteGroup::University, "university name".into()));
    }
    None
}

/// Group for a domain: the user's mapping, then built-in rules, then a cached AI answer, else
/// `Other`.
pub fn decide(domain: &str, entry: Option<&SiteEntry>) -> (SiteGroup, Source) {
    if let Some(group) = entry.and_then(|e| e.user_group) {
        return (group, Source::User);
    }
    if let Some((group, rule)) = rule_group(domain) {
        return (group, Source::Rule(rule));
    }
    if let Some(group) = entry.and_then(|e| e.ai) {
        return (group, Source::Ai);
    }
    (SiteGroup::Other, Source::Default)
}

/// True when the on-device model should be asked about this domain: grouping is on, the user
/// and the rules don't know it, it isn't an IP/localhost and it was never asked.
pub fn wants_ai(settings: &Settings, domain: &str, entry: Option<&SiteEntry>) -> bool {
    settings.site_folders == SiteFolders::GroupAndSite
        && settings.ai_site_groups
        && !is_ip_or_local(domain)
        && entry.is_none_or(|e| e.user_group.is_none() && !e.ai_asked)
        && rule_group(domain).is_none()
}

// ---------- folders ----------

/// Folder-safe version of a user-typed or derived site folder name.
pub fn sanitize_folder(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.').trim();
    let short: String = trimmed.chars().take(100).collect();
    short.trim().to_string()
}

/// Folder name of a site: the user's name for it, else the domain.
pub fn folder_name(domain: &str, entry: Option<&SiteEntry>) -> String {
    entry
        .and_then(|e| e.user_folder.as_deref())
        .map(sanitize_folder)
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| {
            let f = sanitize_folder(domain);
            if f.is_empty() { "site".to_string() } else { f }
        })
}

/// The site levels below the category folder, e.g. `University/sharif.edu` or `sharif.edu`.
pub fn site_subpath(mode: SiteFolders, group: SiteGroup, folder: &str) -> Option<PathBuf> {
    match mode {
        SiteFolders::Off => None,
        SiteFolders::Site => Some(PathBuf::from(folder)),
        SiteFolders::GroupAndSite => Some(PathBuf::from(group.label()).join(folder)),
    }
}

/// Where a file of `category` from this site goes: the category folder (honouring
/// `sort_into_categories` and the per-category override), then the site levels.
pub fn site_dir(
    settings: &Settings,
    category: Category,
    group: SiteGroup,
    folder: &str,
) -> PathBuf {
    let base = categories::dir_for(settings, category);
    match site_subpath(settings.site_folders, group, folder) {
        Some(sub) => base.join(sub),
        None => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dom(host: &str) -> String {
        registrable_domain(host)
    }

    #[test]
    fn registrable_domains() {
        assert_eq!(dom("sharif.edu"), "sharif.edu");
        assert_eq!(dom("www.sharif.edu"), "sharif.edu");
        assert_eq!(dom("lecture.cs.sharif.edu"), "sharif.edu");
        assert_eq!(dom("iust.ac.ir"), "iust.ac.ir");
        assert_eq!(dom("www.iust.ac.ir"), "iust.ac.ir");
        assert_eq!(dom("cdn.files.iust.ac.ir"), "iust.ac.ir");
        assert_eq!(dom("news.bbc.co.uk"), "bbc.co.uk");
        assert_eq!(dom("x.y.example.co.uk"), "example.co.uk");
        assert_eq!(dom("shop.digikala.com"), "digikala.com");
        assert_eq!(dom("EXAMPLE.COM."), "example.com");
        assert_eq!(dom("foo.bar.gov.ir"), "bar.gov.ir");
        assert_eq!(dom("a.b.example.dev"), "example.dev");
        assert_eq!(dom("localhost"), "localhost");
        assert_eq!(dom("api.localhost"), "api.localhost");
        assert_eq!(dom("intranet"), "intranet");
        assert_eq!(dom("drive.google.com"), "drive.google.com");
        assert_eq!(dom("www.google.com"), "google.com");
    }

    #[test]
    fn ips_stay_as_they_are() {
        assert_eq!(dom("127.0.0.1"), "127.0.0.1");
        assert_eq!(dom("192.168.1.20"), "192.168.1.20");
        assert_eq!(dom("::1"), "::1");
    }

    #[test]
    fn site_domain_from_urls() {
        assert_eq!(
            site_domain("http://www.Sharif.edu:8080/a/b.pdf?x=1", None).as_deref(),
            Some("sharif.edu")
        );
        assert_eq!(
            site_domain("http://127.0.0.1:9000/x.zip", None).as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(
            site_domain("http://[::1]:9000/x.zip", None).as_deref(),
            Some("::1")
        );
        assert_eq!(
            site_domain("http://localhost:3000/x.zip", None).as_deref(),
            Some("localhost")
        );
        // IDN hosts stay in their ASCII (punycode) form.
        assert_eq!(
            site_domain("https://www.münchen.de/a.pdf", None).as_deref(),
            Some("xn--mnchen-3ya.de")
        );
        assert_eq!(site_domain("ftp://x.edu/a", None), None);
        assert_eq!(site_domain("not a url", None), None);
    }

    #[test]
    fn referer_is_preferred_over_the_url_host() {
        assert_eq!(
            site_domain(
                "https://cdn.storage-host.net/f/1.pdf",
                Some("https://lms.sharif.edu/course/3")
            )
            .as_deref(),
            Some("sharif.edu")
        );
        // A useless referer falls back to the URL.
        assert_eq!(
            site_domain("https://files.example.org/a.zip", Some("about:blank")).as_deref(),
            Some("example.org")
        );
        assert_eq!(
            site_domain("https://files.example.org/a.zip", Some("")).as_deref(),
            Some("example.org")
        );
    }

    fn group(domain: &str) -> Option<SiteGroup> {
        rule_group(domain).map(|r| r.0)
    }

    #[test]
    fn rule_table() {
        use SiteGroup::*;
        for (d, g) in [
            ("sharif.edu", University),
            ("mit.edu", University),
            ("unimelb.edu.au", University),
            ("iust.ac.ir", University),
            ("ox.ac.uk", University),
            ("uni-heidelberg.de", University),
            ("stanford-university.org", University),
            ("university-of-x.com", University),
            ("irs.gov", Government),
            ("army.mil", Government),
            ("data.gov.uk", Government),
            ("tehran.gov.ir", Government),
            ("github.com", Software),
            ("objects.githubusercontent.com", Software),
            ("gitlab.com", Software),
            ("download.jetbrains.com", Software),
            ("python.org", Software),
            ("developer.apple.com", Software),
            ("drive.google.com", CloudStorage),
            ("docs.google.com", CloudStorage),
            ("dropbox.com", CloudStorage),
            ("mega.nz", CloudStorage),
            ("1drv.ms", CloudStorage),
            ("youtube.com", Video),
            ("vimeo.com", Video),
            ("aparat.com", Video),
            ("soundcloud.com", Music),
            ("open.spotify.com", Music),
            ("arxiv.org", BooksPapers),
            ("archive.org", BooksPapers),
            ("ieee.org", BooksPapers),
            ("amazon.com", Shopping),
            ("digikala.com", Shopping),
            ("x.com", Social),
            ("t.me", Social),
            ("reddit.com", Social),
            ("bbc.co.uk", News),
            ("cnn.com", News),
        ] {
            assert_eq!(group(d), Some(g), "{d}");
        }
    }

    #[test]
    fn rules_match_on_domain_boundaries_and_skip_unknown_sites() {
        assert_eq!(group("notgithub.com"), None);
        assert_eq!(group("box.com"), Some(SiteGroup::CloudStorage));
        assert_eq!(group("fox.com"), None);
        assert_eq!(group("max.com"), None);
        for d in [
            "ostadbank.com",
            "faradars.org",
            "coursera.org",
            "google.com",
            "example.com",
        ] {
            assert_eq!(group(d), None, "{d}");
        }
        assert_eq!(group("127.0.0.1"), None);
        assert_eq!(group("localhost"), None);
        assert_eq!(group("univision.com"), None);
    }

    #[test]
    fn rule_names_are_reported() {
        assert_eq!(rule_group("sharif.edu").unwrap().1, ".edu");
        assert_eq!(rule_group("iust.ac.ir").unwrap().1, ".ac.xx");
        assert_eq!(rule_group("github.com").unwrap().1, "github.com");
    }

    #[test]
    fn user_mapping_wins_then_rules_then_ai_then_other() {
        let mut entry = SiteEntry::default();
        assert_eq!(decide("sharif.edu", Some(&entry)).0, SiteGroup::University);
        entry.user_group = Some(SiteGroup::Software);
        assert_eq!(
            decide("sharif.edu", Some(&entry)),
            (SiteGroup::Software, Source::User)
        );
        // The rules beat a cached AI answer.
        let ai = SiteEntry {
            ai_asked: true,
            ai: Some(SiteGroup::Music),
            ..SiteEntry::default()
        };
        assert_eq!(decide("github.com", Some(&ai)).0, SiteGroup::Software);
        assert_eq!(
            decide("faradars.org", Some(&ai)),
            (SiteGroup::Music, Source::Ai)
        );
        let unsure = SiteEntry {
            ai_asked: true,
            ..SiteEntry::default()
        };
        assert_eq!(
            decide("faradars.org", Some(&unsure)),
            (SiteGroup::Other, Source::Default)
        );
        assert_eq!(decide("faradars.org", None).0, SiteGroup::Other);
    }

    #[test]
    fn ai_is_only_wanted_for_unknown_uncached_real_domains() {
        let on = Settings {
            site_folders: SiteFolders::GroupAndSite,
            ai_site_groups: true,
            ..Settings::default()
        };
        assert!(wants_ai(&on, "faradars.org", None));
        assert!(!wants_ai(&on, "sharif.edu", None));
        assert!(!wants_ai(&on, "127.0.0.1", None));
        assert!(!wants_ai(&on, "localhost", None));
        let cached = SiteEntry {
            ai_asked: true,
            ..SiteEntry::default()
        };
        assert!(!wants_ai(&on, "faradars.org", Some(&cached)));
        let mine = SiteEntry {
            user_group: Some(SiteGroup::News),
            ..SiteEntry::default()
        };
        assert!(!wants_ai(&on, "faradars.org", Some(&mine)));
        let site_only = Settings {
            site_folders: SiteFolders::Site,
            ..on.clone()
        };
        assert!(!wants_ai(&site_only, "faradars.org", None));
        let ai_off = Settings {
            ai_site_groups: false,
            ..on
        };
        assert!(!wants_ai(&ai_off, "faradars.org", None));
    }

    #[test]
    fn folder_names_are_sanitized_and_renamable() {
        assert_eq!(folder_name("sharif.edu", None), "sharif.edu");
        let renamed = SiteEntry {
            user_folder: Some("  Sharif / CS: ..  ".into()),
            ..SiteEntry::default()
        };
        assert_eq!(folder_name("sharif.edu", Some(&renamed)), "Sharif _ CS_ ..");
        let dots = SiteEntry {
            user_folder: Some("../..".into()),
            ..SiteEntry::default()
        };
        // Never an empty name or a path escape.
        assert_eq!(folder_name("sharif.edu", Some(&dots)), "_..");
        let blank = SiteEntry {
            user_folder: Some("   ".into()),
            ..SiteEntry::default()
        };
        assert_eq!(folder_name("sharif.edu", Some(&blank)), "sharif.edu");
        assert_eq!(folder_name("::1", None), "__1");
    }

    fn settings(mode: SiteFolders, sort: bool) -> Settings {
        Settings {
            download_dir: PathBuf::from("/d"),
            sort_into_categories: sort,
            site_folders: mode,
            ..Settings::default()
        }
    }

    #[test]
    fn layout_for_all_modes_sorting_and_overrides() {
        let u = SiteGroup::University;
        let docs = Category::Documents;
        let p = |s: &Settings| site_dir(s, docs, u, "sharif.edu");

        assert_eq!(
            p(&settings(SiteFolders::Off, true)),
            PathBuf::from("/d/Documents")
        );
        assert_eq!(p(&settings(SiteFolders::Off, false)), PathBuf::from("/d"));

        assert_eq!(
            p(&settings(SiteFolders::Site, true)),
            PathBuf::from("/d/Documents/sharif.edu")
        );
        assert_eq!(
            p(&settings(SiteFolders::Site, false)),
            PathBuf::from("/d/sharif.edu")
        );
        assert_eq!(
            p(&settings(SiteFolders::GroupAndSite, true)),
            PathBuf::from("/d/Documents/University/sharif.edu")
        );
        assert_eq!(
            p(&settings(SiteFolders::GroupAndSite, false)),
            PathBuf::from("/d/University/sharif.edu")
        );

        // The site levels go under a per-category folder, with sorting on or off.
        for sort in [true, false] {
            let mut s = settings(SiteFolders::GroupAndSite, sort);
            s.category_dirs
                .insert(Category::Documents, PathBuf::from("/Volumes/Docs"));
            assert_eq!(p(&s), PathBuf::from("/Volumes/Docs/University/sharif.edu"));
            s.site_folders = SiteFolders::Site;
            assert_eq!(p(&s), PathBuf::from("/Volumes/Docs/sharif.edu"));
            // Other categories are unaffected by that override.
            assert_eq!(
                site_dir(&s, Category::Video, u, "sharif.edu"),
                if sort {
                    PathBuf::from("/d/Video/sharif.edu")
                } else {
                    PathBuf::from("/d/sharif.edu")
                }
            );
        }
    }

    #[test]
    fn subpath_matches_the_layout() {
        assert_eq!(
            site_subpath(SiteFolders::Off, SiteGroup::News, "a.com"),
            None
        );
        assert_eq!(
            site_subpath(SiteFolders::GroupAndSite, SiteGroup::BooksPapers, "a.com"),
            Some(PathBuf::from("Books & papers/a.com"))
        );
    }

    #[test]
    fn group_ids_round_trip() {
        for g in SiteGroup::ALL {
            assert_eq!(SiteGroup::from_id(g.id()), Some(g));
            assert_eq!(SiteGroup::from_label(g.label()), Some(g));
        }
        assert_eq!(SiteGroup::from_id("nope"), None);
    }
}
