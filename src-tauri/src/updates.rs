use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri_plugin_opener::OpenerExt;

const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/Cliffwq/otto-sampler/releases/latest";
const RELEASE_PAGE: &str = "https://github.com/Cliffwq/otto-sampler/releases/latest";
const MAX_RELEASE_BYTES: usize = 512 * 1024;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct AvailableUpdate {
    version: String,
    url: String,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
}

fn available_update(current: &str, body: &[u8]) -> Result<Option<AvailableUpdate>, String> {
    let release: GitHubRelease =
        serde_json::from_slice(body).map_err(|error| format!("无法读取更新信息：{error}"))?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let latest = semver::Version::parse(
        release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name),
    )
    .map_err(|error| format!("无效的发布版本：{error}"))?;
    let installed =
        semver::Version::parse(current).map_err(|error| format!("无效的当前版本：{error}"))?;
    if !latest.pre.is_empty() || latest.cmp_precedence(&installed).is_le() {
        return Ok(None);
    }
    Ok(Some(AvailableUpdate {
        version: latest.to_string(),
        url: RELEASE_PAGE.to_string(),
    }))
}

// This only fetches public release metadata. Installation stays with the user.
#[tauri::command]
pub(crate) async fn check_for_updates(
    app: tauri::AppHandle,
) -> Result<Option<AvailableUpdate>, String> {
    let current = app.package_info().version.to_string();
    fetch_update(&current).await
}

async fn fetch_update(current: &str) -> Result<Option<AvailableUpdate>, String> {
    let client = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .user_agent(format!("OTTO-Sampler/{current}"))
        .build()
        .map_err(|error| format!("无法检查更新：{error}"))?;
    let mut response = client
        .get(LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|error| format!("无法连接更新服务：{error}"))?;
    // An empty public repository has no release yet.
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    response = response
        .error_for_status()
        .map_err(|error| format!("更新服务暂时不可用：{error}"))?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RELEASE_BYTES as u64)
    {
        return Err("更新信息过大".to_string());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("无法读取更新信息：{error}"))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RELEASE_BYTES {
            return Err("更新信息过大".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    available_update(current, &body)
}

// Accept no URL from the webview or API response: only the official page opens.
#[tauri::command]
pub(crate) fn open_update_release(app: tauri::AppHandle) -> Result<(), String> {
    app.opener()
        .open_url(RELEASE_PAGE, None::<&str>)
        .map_err(|error| format!("无法打开下载页面：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, draft: bool, prerelease: bool) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "tag_name": tag, "draft": draft, "prerelease": prerelease,
            "html_url": "https://untrusted.example/download"
        }))
        .unwrap()
    }

    #[test]
    fn newer_stable_release_uses_semver_and_official_download_page() {
        let update = available_update("0.1.9", &release("v0.1.10", false, false))
            .unwrap()
            .unwrap();
        assert_eq!(update.version, "0.1.10");
        assert_eq!(update.url, RELEASE_PAGE);
        assert!(available_update("0.1.0", &release("0.2.0", false, false))
            .unwrap()
            .is_some());
    }

    #[test]
    fn same_older_and_build_metadata_versions_are_not_updates() {
        for tag in ["v0.1.0", "v0.0.9", "v0.1.0+build.2"] {
            assert!(available_update("0.1.0", &release(tag, false, false))
                .unwrap()
                .is_none());
        }
    }

    #[test]
    fn draft_and_prerelease_versions_are_not_announced() {
        for (tag, draft, prerelease) in [
            ("v0.2.0", true, false),
            ("v0.2.0", false, true),
            ("v0.2.0-beta.1", false, false),
        ] {
            assert!(available_update("0.1.0", &release(tag, draft, prerelease))
                .unwrap()
                .is_none());
        }
    }

    #[test]
    fn malformed_response_or_version_returns_error() {
        for body in [b"not json".as_slice(), b"{}".as_slice()] {
            assert!(available_update("0.1.0", body).is_err());
        }
        assert!(available_update("0.1.0", &release("vnext", false, false)).is_err());
        assert!(available_update("invalid", &release("v0.2.0", false, false)).is_err());
    }

    #[test]
    #[ignore = "requires access to the public GitHub release API"]
    fn live_github_release_check() {
        let result = tauri::async_runtime::block_on(fetch_update("0.0.0")).unwrap();
        let update = result.expect("public repository has a stable release");
        assert_eq!(update.url, RELEASE_PAGE);
        assert!(semver::Version::parse(&update.version).is_ok());
    }
}
