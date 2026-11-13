//! Download a Minecraft version into a game directory.

use crate::assets::{self, reconstruct_assets};
use crate::download::{self, sha1_hex, RemoteFile};
use crate::error::{Error, Result};
use crate::java::{self, JavaInstallation, JavaMode};
use crate::library::{self, plan_libraries};
use crate::manifest;
use crate::paths::GamePaths;
use crate::platform::OsInfo;
use crate::progress::ProgressSink;
use crate::rules::FeatureSet;
use crate::version::{self, VersionInfo};

/// What to install, and for which operating system.
#[derive(Debug, Clone)]
pub struct InstallOptions {
    /// Version id, or `release` / `snapshot`.
    pub version: String,
    /// OS whose libraries and natives should be installed.
    pub os: OsInfo,
    /// Maximum concurrent downloads.
    pub concurrency: usize,
    /// Whether to download Mojang's Java runtime.
    pub java: JavaMode,
}

/// What an install wrote.
#[derive(Debug, Clone)]
pub struct InstallReport {
    /// Installed version id.
    pub version_id: String,
    /// Release, snapshot, or an older channel.
    pub version_type: String,
    /// Client jar path.
    pub client_jar: std::path::PathBuf,
    /// Library jars placed on the classpath.
    pub libraries: usize,
    /// Native jars extracted.
    pub natives: usize,
    /// Asset objects in the index.
    pub assets: usize,
    /// Files transferred during this run.
    pub files_downloaded: u64,
    /// Files already present and valid.
    pub files_cached: u64,
    /// Java runtime that will launch this version, when one was resolved.
    pub java: Option<JavaInstallation>,
    /// Short note about the Java runtime.
    pub java_note: String,
    /// The manifest was served from the local cache.
    pub used_cached_manifest: bool,
}

/// Download the client, libraries, natives, assets, and (when needed) Java.
pub async fn install_version(
    client: &reqwest::Client,
    paths: &GamePaths,
    options: &InstallOptions,
    progress: &ProgressSink,
) -> Result<InstallReport> {
    tokio::fs::create_dir_all(&paths.root)
        .await
        .map_err(|err| Error::io(&paths.root, err))?;
    let fetched = manifest::fetch_manifest(client, &paths.manifest_cache()).await?;
    let version_id = fetched.manifest.resolve_id(&options.version).to_string();
    let resolved =
        version::load_resolved(client, paths, &fetched.manifest, &version_id, true).await?;

    let mut downloaded = 0u64;
    let mut cached = 0u64;

    let (client_jar, client_stats) = install_client(
        client,
        paths,
        &resolved.info,
        &resolved.jar_id,
        options.concurrency,
        progress,
    )
    .await?;
    downloaded += client_stats.downloaded;
    cached += client_stats.skipped;

    let libraries = resolved.info.libraries.clone().unwrap_or_default();
    let library_plan = plan_libraries(
        &libraries,
        &paths.libraries(),
        &options.os,
        &FeatureSet::default(),
    )?;
    let stats = download::download_all(
        client,
        library_plan.downloads.clone(),
        options.concurrency,
        "libraries",
        progress,
    )
    .await?;
    downloaded += stats.downloaded;
    cached += stats.skipped;

    let natives = library_plan.natives.clone();
    let natives_dir = paths.natives_dir(&version_id);
    let extracted = natives.len();
    tokio::task::spawn_blocking(move || -> Result<()> {
        library::reset_dir(&natives_dir)?;
        for native in &natives {
            library::extract_native_jar(&native.jar, &natives_dir, &native.exclude)?;
        }
        Ok(())
    })
    .await
    .map_err(|err| Error::message(format!("could not extract natives: {err}")))??;

    let asset_count = install_assets(
        client,
        paths,
        &resolved.info,
        options.concurrency,
        progress,
        &mut downloaded,
        &mut cached,
    )
    .await?;
    install_logging(
        client,
        paths,
        &resolved.info,
        options.concurrency,
        progress,
        &mut downloaded,
        &mut cached,
    )
    .await?;

    let (java, java_note) = match options.java {
        JavaMode::Skip => (None, "skipped (--no-java)".to_string()),
        mode => {
            let (installation, java_stats) = java::resolve_java(
                client,
                paths,
                java::JavaRequest {
                    spec: resolved.info.java_version.as_ref(),
                    explicit: None,
                    mode,
                    os: &options.os,
                    concurrency: options.concurrency,
                },
                progress,
            )
            .await?;
            downloaded += java_stats.downloaded;
            cached += java_stats.skipped;
            let note = format!(
                "Java {} at {} ({})",
                installation.version,
                installation.path.display(),
                installation.source
            );
            (Some(installation), note)
        }
    };

    Ok(InstallReport {
        version_id: resolved.info.id,
        version_type: resolved
            .info
            .version_type
            .unwrap_or_else(|| "unknown".into()),
        client_jar,
        libraries: library_plan.classpath.len(),
        natives: extracted,
        assets: asset_count,
        files_downloaded: downloaded,
        files_cached: cached,
        java,
        java_note,
        used_cached_manifest: fetched.from_cache,
    })
}

async fn install_client(
    client: &reqwest::Client,
    paths: &GamePaths,
    version: &VersionInfo,
    jar_id: &str,
    concurrency: usize,
    progress: &ProgressSink,
) -> Result<(std::path::PathBuf, download::DownloadStats)> {
    let artifact = version
        .downloads
        .as_ref()
        .and_then(|downloads| downloads.client.as_ref())
        .ok_or_else(|| Error::MissingClient {
            id: jar_id.to_string(),
        })?;
    let url = artifact.url().ok_or_else(|| Error::MissingClient {
        id: jar_id.to_string(),
    })?;
    let dest = paths.version_jar(jar_id);
    let stats = download::download_all(
        client,
        vec![RemoteFile {
            url: url.to_string(),
            dest: dest.clone(),
            sha1: artifact.sha1.clone(),
            size: artifact.size,
            executable: false,
        }],
        concurrency,
        "client",
        progress,
    )
    .await?;
    Ok((dest, stats))
}

async fn install_assets(
    client: &reqwest::Client,
    paths: &GamePaths,
    version: &VersionInfo,
    concurrency: usize,
    progress: &ProgressSink,
    downloaded: &mut u64,
    cached: &mut u64,
) -> Result<usize> {
    let Some(index_ref) = version.asset_index.as_ref() else {
        return Ok(0);
    };
    let index_name = version::asset_index_name(version).unwrap_or_else(|| index_ref.id.clone());
    let bytes = download::get_bytes(client, &index_ref.url).await?;
    let actual = sha1_hex(&bytes);
    if !actual.eq_ignore_ascii_case(&index_ref.sha1) {
        return Err(Error::Checksum {
            path: paths.asset_index(&index_name),
            expected: index_ref.sha1.clone(),
            actual,
        });
    }
    let index_path = paths.asset_index(&index_name);
    download::write_atomic(&index_path, &bytes).await?;
    let index = assets::parse_asset_index(&bytes)?;
    let files = assets::plan_asset_downloads(&index, &paths.asset_objects())?;
    let count = index.objects.len();
    let stats = download::download_all(client, files, concurrency, "assets", progress).await?;
    *downloaded += stats.downloaded;
    *cached += stats.skipped;
    if index.map_to_resources {
        reconstruct_assets(&index, &paths.asset_objects(), &paths.resources())?;
    }
    if index.virtual_assets {
        reconstruct_assets(
            &index,
            &paths.asset_objects(),
            &paths.virtual_assets(&index_name),
        )?;
    }
    Ok(count)
}

async fn install_logging(
    client: &reqwest::Client,
    paths: &GamePaths,
    version: &VersionInfo,
    concurrency: usize,
    progress: &ProgressSink,
    downloaded: &mut u64,
    cached: &mut u64,
) -> Result<()> {
    let Some(logging) = version
        .logging
        .as_ref()
        .and_then(|logging| logging.client.as_ref())
    else {
        return Ok(());
    };
    let stats = download::download_all(
        client,
        vec![RemoteFile {
            url: logging.file.url.clone(),
            dest: paths.log_config(&logging.file.id),
            sha1: Some(logging.file.sha1.clone()),
            size: Some(logging.file.size),
            executable: false,
        }],
        concurrency,
        "logging",
        progress,
    )
    .await?;
    *downloaded += stats.downloaded;
    *cached += stats.skipped;
    Ok(())
}
