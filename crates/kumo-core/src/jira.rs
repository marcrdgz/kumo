//! Minimal Jira Cloud issue lookup used by task-oriented worktree creation.

use std::time::Duration;

use base64::Engine as _;
use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JiraIssue {
    pub key: String,
    pub summary: String,
    pub status: Option<String>,
    pub url: String,
}

#[derive(Deserialize)]
struct IssueResponse {
    key: String,
    fields: IssueFields,
}

#[derive(Deserialize)]
struct IssueFields {
    summary: String,
    #[serde(default)]
    status: Option<IssueStatus>,
}

#[derive(Deserialize)]
struct IssueStatus {
    name: String,
}

pub fn issue_key(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let browse = trimmed.split_once("/browse/");
    let candidate = browse
        .map(|(_, tail)| tail.split(['/', '?', '#']).next().unwrap_or(""))
        .unwrap_or(trimmed);
    let (project, number) = candidate.split_once('-')?;
    (project.len() >= 2
        && project.len() <= 20
        && project.chars().all(|ch| ch.is_ascii_alphanumeric())
        && (browse.is_some() || project.chars().all(|ch| !ch.is_ascii_alphabetic() || ch.is_ascii_uppercase()))
        && !number.is_empty()
        && number.chars().all(|ch| ch.is_ascii_digit()))
    .then(|| candidate.to_ascii_uppercase())
}

fn site_from_url(raw: &str) -> Option<String> {
    let scheme_end = raw.find("://")? + 3;
    let host_end = raw[scheme_end..].find('/').map(|offset| scheme_end + offset).unwrap_or(raw.len());
    Some(raw[..host_end].trim_end_matches('/').to_string())
}

pub fn fetch_issue(raw: &str, configured_site: Option<&str>, email: &str, token: &str) -> Result<JiraIssue, String> {
    let key = issue_key(raw).ok_or_else(|| format!("invalid Jira issue reference {raw:?}"))?;
    let site = site_from_url(raw)
        .or_else(|| configured_site.map(|value| value.trim().trim_end_matches('/').to_string()))
        .filter(|value| value.starts_with("https://"))
        .ok_or_else(|| "Jira site is missing; configure [jira] site or use a full issue URL".to_string())?;
    if email.trim().is_empty() || token.trim().is_empty() {
        return Err("Jira credentials are missing; configure [jira] email and KUMO_JIRA_API_TOKEN".to_string());
    }

    let auth = base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", email.trim(), token.trim()));
    let endpoint = format!("{site}/rest/api/3/issue/{key}?fields=summary,status");
    let mut response = ureq::get(&endpoint)
        .header("Accept", "application/json")
        .header("Authorization", format!("Basic {auth}"))
        .config()
        .timeout_global(Some(Duration::from_secs(10)))
        .user_agent(format!("kumo/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .call()
        .map_err(|error| format!("Jira request failed for {key}: {error}"))?;
    let body = response.body_mut().read_to_string().map_err(|error| format!("Jira response read failed: {error}"))?;
    let issue: IssueResponse = serde_json::from_str(&body).map_err(|error| format!("invalid Jira response: {error}"))?;
    Ok(JiraIssue {
        key: issue.key,
        summary: issue.fields.summary,
        status: issue.fields.status.map(|status| status.name),
        url: format!("{site}/browse/{key}"),
    })
}

#[cfg(test)]
mod tests {
    use super::{fetch_issue, issue_key};

    #[test]
    fn parses_jira_keys_and_browse_urls() {
        assert_eq!(issue_key("PROJ-123"), Some("PROJ-123".into()));
        assert_eq!(issue_key("feat-123"), None);
        assert_eq!(issue_key("https://acme.atlassian.net/browse/ABC-42?focused=true"), Some("ABC-42".into()));
        assert_eq!(issue_key("not an issue"), None);
    }

    #[test]
    fn lookup_requires_credentials_before_network_access() {
        let error = fetch_issue("PROJ-123", Some("https://acme.atlassian.net"), "", "")
            .unwrap_err();
        assert!(error.contains("credentials"));
    }
}
