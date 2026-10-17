//! The forge, GitHub only (worker-interface.md §3, §6, §7): pull requests from the mirror's
//! branches, their checks as the environment's evidence, and adoption as a merge pinned to a
//! commit. The worker host authenticates as a GitHub App, or with a token it is given; the agent
//! never sees either.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use base64::Engine as _;
use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

/// How the worker host signs in to GitHub.
#[derive(Clone)]
pub enum Auth {
    /// A GitHub App installed on the repository: tokens are minted for it, scoped to the
    /// repository, for an hour (tech-plan.md §3).
    App { id: u64, key: PathBuf },
    /// A token as it is, such as a test's.
    Token(String),
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Auth::App { id, key } => write!(f, "App {{ id: {id}, key: {} }}", key.display()),
            Auth::Token(_) => write!(f, "Token(<hidden>)"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    /// Such as `https://api.github.com`.
    pub api: String,
    pub owner: String,
    pub repo: String,
    pub auth: Auth,
    /// Who the squash commit names as co-author, such as `Name <email>`.
    pub co_author: Option<String>,
}

pub struct GitHub {
    pub config: Config,
    http: reqwest::Client,
    /// The installation token and when to mint a new one.
    token: Mutex<Option<(String, Instant)>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PullRequest {
    pub number: u64,
    pub url: String,
}

/// A check run on a commit.
#[derive(Clone, Debug, Deserialize)]
pub struct Check {
    pub id: u64,
    pub name: String,
    /// `queued`, `in_progress` or `completed`.
    pub status: String,
    pub conclusion: Option<String>,
    pub html_url: Option<String>,
}

impl Check {
    pub fn passed(&self) -> Option<bool> {
        (self.status == "completed")
            .then_some(matches!(self.conclusion.as_deref(), Some("success" | "neutral" | "skipped")))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Merge {
    Merged,
    /// The pull request's head is no longer the commit asked for.
    HeadChanged,
    /// GitHub would not merge it, for the reason it gave.
    Refused(String),
}

/// How much of a failed check's log goes back to the agent.
const LOG_LINES: usize = 80;
const LOG_CHARS: usize = 6000;

impl GitHub {
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder().user_agent("lobotomy-worker").timeout(Duration::from_secs(60)).build()?;
        Ok(Self { config, http, token: Mutex::new(None) })
    }

    /// A token for the repository, minted again ten minutes before it expires.
    pub async fn token(&self) -> anyhow::Result<String> {
        let (id, key) = match &self.config.auth {
            Auth::Token(token) => return Ok(token.clone()),
            Auth::App { id, key } => (*id, key.clone()),
        };
        if let Some((token, until)) = self.token.lock().unwrap().clone()
            && Instant::now() < until
        {
            return Ok(token);
        }
        let jwt = app_jwt(id, &key)?;
        let path = format!("/repos/{}/{}/installation", self.config.owner, self.config.repo);
        let installation: Value = self.send(Method::GET, &path, None, &jwt).await?.1;
        let installation = installation["id"].as_u64().context("GitHub gave no installation id")?;
        let body = json!({ "repositories": [self.config.repo] });
        let path = format!("/app/installations/{installation}/access_tokens");
        let minted = self.send(Method::POST, &path, Some(body), &jwt).await?.1;
        let token = minted["token"].as_str().context("GitHub gave no token")?.to_owned();
        *self.token.lock().unwrap() = Some((token.clone(), Instant::now() + Duration::from_secs(50 * 60)));
        Ok(token)
    }

    /// The variables that give git the token for one command, outside its arguments and URLs.
    pub async fn git_env(&self) -> anyhow::Result<Vec<(String, String)>> {
        let token = self.token().await?;
        let basic = base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"));
        Ok(vec![
            ("GIT_CONFIG_COUNT".into(), "2".into()),
            ("GIT_CONFIG_KEY_0".into(), "credential.helper".into()),
            ("GIT_CONFIG_VALUE_0".into(), String::new()),
            ("GIT_CONFIG_KEY_1".into(), "http.extraHeader".into()),
            ("GIT_CONFIG_VALUE_1".into(), format!("Authorization: Basic {basic}")),
        ])
    }

    /// The open pull request from `branch`, made if there is none.
    pub async fn open_pull_request(
        &self,
        branch: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> anyhow::Result<PullRequest> {
        let (owner, repo) = (&self.config.owner, &self.config.repo);
        let path = format!("/repos/{owner}/{repo}/pulls?state=open&head={owner}:{branch}");
        let found = self.call(Method::GET, &path, None).await?;
        if let Some(pr) = found.as_array().and_then(|prs| prs.first()) {
            return pull_request(pr);
        }
        let body = json!({ "title": title, "head": branch, "base": base, "body": body });
        pull_request(&self.call(Method::POST, &format!("/repos/{owner}/{repo}/pulls"), Some(body)).await?)
    }

    pub async fn close_pull_request(&self, number: u64) -> anyhow::Result<()> {
        let path = format!("/repos/{}/{}/pulls/{number}", self.config.owner, self.config.repo);
        self.call(Method::PATCH, &path, Some(json!({ "state": "closed" }))).await?;
        Ok(())
    }

    /// The check runs on a commit.
    pub async fn checks(&self, sha: &str) -> anyhow::Result<Vec<Check>> {
        let path = format!("/repos/{}/{}/commits/{sha}/check-runs?per_page=100", self.config.owner, self.config.repo);
        let found = self.call(Method::GET, &path, None).await?;
        serde_json::from_value(found["check_runs"].clone()).context("reading the check runs")
    }

    /// The end of a failed check's log, as the agent gets it to fix the failure. A check that is
    /// not an Actions job has none.
    pub async fn failure_log(&self, check: &Check) -> anyhow::Result<String> {
        let path = format!("/repos/{}/{}/actions/jobs/{}/logs", self.config.owner, self.config.repo, check.id);
        let token = self.token().await?;
        let response = self.request(Method::GET, &path, &token).send().await?;
        if !response.status().is_success() {
            bail!("no log for {}: {}", check.name, response.status());
        }
        let text = response.text().await?;
        let lines: Vec<&str> = text.lines().collect();
        let tail = lines[lines.len().saturating_sub(LOG_LINES)..].join("\n");
        let start = tail.len().saturating_sub(LOG_CHARS);
        let start = (start..tail.len()).find(|&i| tail.is_char_boundary(i)).unwrap_or(tail.len());
        Ok(tail[start..].to_owned())
    }

    /// Squash-merges the pull request if its head is still `sha` (worker-interface.md §6).
    pub async fn merge(&self, number: u64, sha: &str, title: &str, message: &str) -> anyhow::Result<Merge> {
        let path = format!("/repos/{}/{}/pulls/{number}/merge", self.config.owner, self.config.repo);
        let mut message = message.to_owned();
        if let Some(co_author) = &self.config.co_author {
            message.push_str(&format!("\n\nCo-authored-by: {co_author}"));
        }
        let body = json!({ "sha": sha, "merge_method": "squash", "commit_title": title, "commit_message": message });
        let token = self.token().await?;
        let response = self.request(Method::PUT, &path, &token).json(&body).send().await?;
        let status = response.status();
        let reply: Value = response.json().await.unwrap_or(Value::Null);
        Ok(match status {
            s if s.is_success() => Merge::Merged,
            StatusCode::CONFLICT => Merge::HeadChanged,
            _ => Merge::Refused(format!("{status}: {}", reply["message"].as_str().unwrap_or_default())),
        })
    }

    pub async fn delete_branch(&self, branch: &str) -> anyhow::Result<()> {
        let path = format!("/repos/{}/{}/git/refs/heads/{branch}", self.config.owner, self.config.repo);
        self.call(Method::DELETE, &path, None).await?;
        Ok(())
    }

    async fn call(&self, method: Method, path: &str, body: Option<Value>) -> anyhow::Result<Value> {
        let token = self.token().await?;
        Ok(self.send(method, path, body, &token).await?.1)
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        bearer: &str,
    ) -> anyhow::Result<(StatusCode, Value)> {
        let mut request = self.request(method.clone(), path, bearer);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.with_context(|| format!("{method} {path}"))?;
        let status = response.status();
        let reply: Value = match response.text().await? {
            text if text.trim().is_empty() => Value::Null,
            text => serde_json::from_str(&text).unwrap_or(Value::String(text)),
        };
        if !status.is_success() {
            bail!("{method} {path}: {status}: {}", reply["message"].as_str().unwrap_or_default());
        }
        Ok((status, reply))
    }

    fn request(&self, method: Method, path: &str, bearer: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.config.api))
            .bearer_auth(bearer)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
    }
}

fn pull_request(pr: &Value) -> anyhow::Result<PullRequest> {
    Ok(PullRequest {
        number: pr["number"].as_u64().context("a pull request without a number")?,
        url: pr["html_url"].as_str().context("a pull request without a URL")?.to_owned(),
    })
}

/// The App's own token, which only mints installation tokens: signed with its private key, valid
/// for nine minutes, issued a minute early against clock drift.
fn app_jwt(id: u64, key: &std::path::Path) -> anyhow::Result<String> {
    let pem = std::fs::read(key).with_context(|| format!("reading the App's key at {}", key.display()))?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let claims = json!({ "iat": now - 60, "exp": now + 540, "iss": id });
    let key = jsonwebtoken::EncodingKey::from_rsa_pem(&pem).context("the App's key is not an RSA key in PEM")?;
    Ok(jsonwebtoken::encode(&jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256), &claims, &key)?)
}
