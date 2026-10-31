//! Site folders end to end: the Referer decides the folder, user mappings and renames apply,
//! and the optional AI helper (a fake script here) is asked once per unknown domain.
mod common;
use common::*;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use sushload_core::{AddRequest, Engine, SiteFolders, SiteGroup};

const T20: Duration = Duration::from_secs(20);

fn pdf(server: &TestServer, name: &str, referer: Option<&str>) -> AddRequest {
    AddRequest {
        url: server.url(name),
        file_name: Some(name.to_string()),
        referer: referer.map(str::to_string),
        ..AddRequest::default()
    }
}

fn settings_with(dirs: &Dirs, mode: SiteFolders) -> sushload_core::Settings {
    sushload_core::Settings {
        sort_into_categories: true,
        site_folders: mode,
        ai_site_groups: false,
        ..dirs.settings()
    }
}

async fn download(engine: &Engine, req: AddRequest) -> PathBuf {
    let id = engine.add_request(req).expect("add");
    wait_completed(engine, id, T20).await.path
}

fn fake_helper(dir: &Path, log: &Path, group: &str) -> PathBuf {
    let path = dir.join("tgsort-ai");
    let body = format!(
        r#"#!/bin/sh
if [ "$1" = "--check" ]; then echo '{{"available":true}}'; exit 0; fi
echo "$(date +%s)" >> '{log}'
cat > '{log}.in'
echo '{{"results":[{{"id":"0","choice":{group}}}]}}'
"#,
        log = log.display(),
    );
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

async fn wait_ai(engine: &Engine, domain: &str) -> sushload_core::SiteView {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(s) = engine
            .sites()
            .into_iter()
            .find(|s| s.domain == domain && s.origin == "ai")
        {
            return s;
        }
        assert!(Instant::now() < end, "AI never answered for {domain}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn referer_university_lands_in_group_and_site_folders() {
    let server = start_server().await;
    let dirs = Dirs::new();
    let engine = dirs.engine(settings_with(&dirs, SiteFolders::GroupAndSite));

    let path = download(
        &engine,
        pdf(
            &server,
            "lecture-03.pdf",
            Some("https://lms.cs.sharif.edu/course/3"),
        ),
    )
    .await;
    assert_eq!(
        path,
        dirs.downloads
            .path()
            .join("Documents/University/sharif.edu/lecture-03.pdf")
    );
    assert!(path.is_file());

    // Without a Referer the URL's host (127.0.0.1 here) is the site, and IPs are never grouped.
    let path = download(&engine, pdf(&server, "plain.pdf", None)).await;
    assert_eq!(
        path,
        dirs.downloads
            .path()
            .join("Documents/Other/127.0.0.1/plain.pdf")
    );

    let seen: Vec<_> = engine.sites().into_iter().map(|s| s.domain).collect();
    assert_eq!(seen, ["127.0.0.1", "sharif.edu"]);
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn modes_and_explicit_dir() {
    let server = start_server().await;
    let dirs = Dirs::new();
    let engine = dirs.engine(settings_with(&dirs, SiteFolders::Site));
    let referer = Some("https://www.iust.ac.ir/fa");

    let path = download(&engine, pdf(&server, "a.pdf", referer)).await;
    assert_eq!(
        path,
        dirs.downloads.path().join("Documents/iust.ac.ir/a.pdf")
    );

    let mut s = settings_with(&dirs, SiteFolders::GroupAndSite);
    s.sort_into_categories = false;
    engine.set_settings(s.clone());
    let path = download(&engine, pdf(&server, "b.pdf", referer)).await;
    assert_eq!(
        path,
        dirs.downloads.path().join("University/iust.ac.ir/b.pdf")
    );

    s.site_folders = SiteFolders::Off;
    engine.set_settings(s);
    let path = download(&engine, pdf(&server, "c.pdf", referer)).await;
    assert_eq!(path, dirs.downloads.path().join("c.pdf"));

    // The dialog's folder wins over everything.
    engine.set_settings(settings_with(&dirs, SiteFolders::GroupAndSite));
    let target = tempfile::tempdir().unwrap();
    let path = download(
        &engine,
        AddRequest {
            dir: Some(target.path().to_path_buf()),
            ..pdf(&server, "d.pdf", referer)
        },
    )
    .await;
    assert_eq!(path, target.path().join("d.pdf"));
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probe_and_subpath_show_the_site_folder() {
    let server = start_server().await;
    let dirs = Dirs::new();
    let engine = dirs.engine(settings_with(&dirs, SiteFolders::GroupAndSite));
    let req = pdf(&server, "p.pdf", Some("https://sharif.edu/x"));
    assert_eq!(
        engine.site_subpath(&req),
        Some(PathBuf::from("University/sharif.edu"))
    );
    // Asking is free of side effects.
    assert!(engine.sites().is_empty());

    let e2 = engine.clone();
    let r2 = req.clone();
    let info = tokio::task::spawn_blocking(move || e2.probe(&r2))
        .await
        .unwrap()
        .expect("probe");
    assert_eq!(
        info.dir,
        dirs.downloads
            .path()
            .join("Documents/University/sharif.edu")
    );
    engine.set_settings(settings_with(&dirs, SiteFolders::Off));
    assert_eq!(engine.site_subpath(&req), None);
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_mapping_and_folder_name_win_and_persist() {
    let server = start_server().await;
    let dirs = Dirs::new();
    let engine = dirs.engine(settings_with(&dirs, SiteFolders::GroupAndSite));
    let referer = Some("https://sharif.edu/");

    download(&engine, pdf(&server, "one.pdf", referer)).await;
    engine
        .set_site(
            "sharif.edu",
            Some(SiteGroup::Software),
            Some("Sharif CS".into()),
        )
        .unwrap();
    let path = download(&engine, pdf(&server, "two.pdf", referer)).await;
    assert_eq!(
        path,
        dirs.downloads
            .path()
            .join("Documents/Software/Sharif CS/two.pdf")
    );
    let row = &engine.sites()[0];
    assert_eq!((row.origin, row.folder.as_str()), ("you", "Sharif CS"));

    engine.shutdown();
    let engine = Engine::start(dirs.data.path().to_path_buf()).expect("restart");
    let row = &engine.sites()[0];
    assert_eq!(row.user_group, Some(SiteGroup::Software));
    assert_eq!(row.user_folder.as_deref(), Some("Sharif CS"));

    engine.reset_site("sharif.edu");
    let path = download(&engine, pdf(&server, "three.pdf", referer)).await;
    assert_eq!(
        path,
        dirs.downloads
            .path()
            .join("Documents/University/sharif.edu/three.pdf")
    );
    assert_eq!(engine.sites()[0].origin, "auto");
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ai_is_asked_once_per_unknown_domain_and_cached() {
    let server = start_server().await;
    let dirs = Dirs::new();
    let work = tempfile::tempdir().unwrap();
    let log = work.path().join("calls.log");
    let engine = dirs.engine(sushload_core::Settings {
        ai_site_groups: true,
        ..settings_with(&dirs, SiteFolders::GroupAndSite)
    });
    engine.set_ai_helper(Some(fake_helper(work.path(), &log, r#""Software""#)));
    let referer = Some("https://www.faradars.org/courses/py");

    // Until it answers the site is filed under Other.
    let first = download(&engine, pdf(&server, "a.pdf", referer)).await;
    assert_eq!(
        first,
        dirs.downloads
            .path()
            .join("Documents/Other/faradars.org/a.pdf")
    );
    let row = wait_ai(&engine, "faradars.org").await;
    assert_eq!(row.group, SiteGroup::Software);

    let second = download(&engine, pdf(&server, "b.pdf", referer)).await;
    assert_eq!(
        second,
        dirs.downloads
            .path()
            .join("Documents/Software/faradars.org/b.pdf")
    );
    download(&engine, pdf(&server, "c.pdf", referer)).await;
    let calls = std::fs::read_to_string(&log).unwrap();
    assert_eq!(calls.lines().count(), 1, "helper ran more than once");
    // The helper saw a description of the site, not a file of the user's.
    let seen = std::fs::read_to_string(format!("{}.in", log.display())).unwrap();
    assert!(seen.contains("sushload-site-"));

    // The answer survives a restart and is not asked again.
    engine.shutdown();
    let engine = Engine::start(dirs.data.path().to_path_buf()).expect("restart");
    engine.set_ai_helper(Some(fake_helper(work.path(), &log, r#""Software""#)));
    download(&engine, pdf(&server, "d.pdf", referer)).await;
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 1);
    engine.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ai_unsure_is_cached_and_off_or_known_domains_skip_it() {
    let server = start_server().await;
    let dirs = Dirs::new();
    let work = tempfile::tempdir().unwrap();
    let log = work.path().join("calls.log");
    let mut s = settings_with(&dirs, SiteFolders::GroupAndSite);
    s.ai_site_groups = true;
    let engine = dirs.engine(s.clone());
    engine.set_ai_helper(Some(fake_helper(work.path(), &log, "null")));

    // A known domain, an IP and (with the toggle off) an unknown domain never reach the helper.
    download(&engine, pdf(&server, "a.pdf", Some("https://github.com/x"))).await;
    download(&engine, pdf(&server, "b.pdf", None)).await;
    s.ai_site_groups = false;
    engine.set_settings(s.clone());
    download(
        &engine,
        pdf(&server, "c.pdf", Some("https://ostadbank.com/")),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!log.exists());

    // Unsure: asked once, stays Other, not asked again.
    s.ai_site_groups = true;
    engine.set_settings(s);
    let other = Some("https://coursera.org/learn");
    download(&engine, pdf(&server, "d.pdf", other)).await;
    let end = Instant::now() + Duration::from_secs(10);
    while !log.exists() {
        assert!(Instant::now() < end, "helper never ran");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let path = download(&engine, pdf(&server, "e.pdf", other)).await;
    assert_eq!(
        path,
        dirs.downloads
            .path()
            .join("Documents/Other/coursera.org/e.pdf")
    );
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 1);
    let row = engine
        .sites()
        .into_iter()
        .find(|s| s.domain == "coursera.org")
        .unwrap();
    assert_eq!((row.group, row.origin), (SiteGroup::Other, "auto"));

    // "Unsure" is remembered across a restart too: the helper is not asked again.
    engine.shutdown();
    let engine = Engine::start(dirs.data.path().to_path_buf()).expect("restart");
    engine.set_ai_helper(Some(fake_helper(work.path(), &log, "null")));
    let mut s2 = engine.settings();
    s2.ai_site_groups = true;
    engine.set_settings(s2);
    download(&engine, pdf(&server, "f.pdf", other)).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 1);
    engine.shutdown();
}
