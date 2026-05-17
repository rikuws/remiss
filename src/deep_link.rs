use std::{cell::RefCell, rc::Rc};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeepLinkRequest {
    GitHubPullRequest { repository: String, number: i64 },
}

#[derive(Clone, Default)]
pub struct DeepLinkDispatcher {
    state: Rc<RefCell<DeepLinkDispatcherState>>,
}

#[derive(Default)]
struct DeepLinkDispatcherState {
    handler: Option<Box<dyn FnMut(DeepLinkRequest)>>,
    pending_urls: Vec<String>,
}

impl DeepLinkDispatcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn receive_urls(&self, urls: Vec<String>) {
        let mut state = self.state.borrow_mut();
        if state.handler.is_none() {
            state.pending_urls.extend(urls);
            return;
        }

        deliver_urls(&mut state, urls);
    }

    pub fn install_handler(&self, handler: impl FnMut(DeepLinkRequest) + 'static) {
        let pending_urls = {
            let mut state = self.state.borrow_mut();
            state.handler = Some(Box::new(handler));
            std::mem::take(&mut state.pending_urls)
        };

        if !pending_urls.is_empty() {
            self.receive_urls(pending_urls);
        }
    }
}

pub fn parse_url(url: &str) -> Result<DeepLinkRequest, String> {
    let trimmed = url.trim();
    let rest = trimmed
        .strip_prefix("remiss://")
        .ok_or_else(|| "Remiss links must start with remiss://".to_string())?;
    let rest = rest
        .split(['?', '#'])
        .next()
        .unwrap_or(rest)
        .trim_matches('/');
    let parts = rest
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();

    match parts.as_slice() {
        ["github" | "github.com", owner, repo, "pull", number] => {
            let number = number
                .parse::<i64>()
                .map_err(|_| format!("Invalid pull request number '{number}'."))?;
            if number <= 0 {
                return Err("Pull request number must be positive.".to_string());
            }

            Ok(DeepLinkRequest::GitHubPullRequest {
                repository: format!("{owner}/{repo}"),
                number,
            })
        }
        _ => Err(format!("Unsupported Remiss link '{trimmed}'.")),
    }
}

pub fn github_pull_request_web_url(repository: &str, number: i64) -> String {
    format!("https://github.com/{repository}/pull/{number}")
}

fn deliver_urls(state: &mut DeepLinkDispatcherState, urls: Vec<String>) {
    let Some(handler) = state.handler.as_mut() else {
        state.pending_urls.extend(urls);
        return;
    };

    for url in urls {
        match parse_url(&url) {
            Ok(request) => handler(request),
            Err(error) => eprintln!("Ignoring Remiss URL '{url}': {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_url, DeepLinkRequest};

    #[test]
    fn parses_github_pull_request_links() {
        assert_eq!(
            parse_url("remiss://github/rikuws/remiss/pull/17").unwrap(),
            DeepLinkRequest::GitHubPullRequest {
                repository: "rikuws/remiss".to_string(),
                number: 17,
            }
        );
    }

    #[test]
    fn accepts_github_dot_com_host_and_ignores_query() {
        assert_eq!(
            parse_url("remiss://github.com/rikuws/remiss/pull/17?tab=files#discussion").unwrap(),
            DeepLinkRequest::GitHubPullRequest {
                repository: "rikuws/remiss".to_string(),
                number: 17,
            }
        );
    }

    #[test]
    fn rejects_unsupported_routes() {
        assert!(parse_url("remiss://github/rikuws/remiss/issues/17").is_err());
        assert!(parse_url("https://github.com/rikuws/remiss/pull/17").is_err());
    }
}
