use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

const DUPLICATE_URL_WINDOW: Duration = Duration::from_secs(2);

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
    recently_received_urls: Vec<(String, Instant)>,
}

impl DeepLinkDispatcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn receive_urls(&self, urls: Vec<String>) {
        let mut state = self.state.borrow_mut();
        let urls = urls
            .into_iter()
            .filter(|url| state.remember_url(url))
            .collect::<Vec<_>>();
        if urls.is_empty() {
            return;
        }

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
            deliver_urls(&mut self.state.borrow_mut(), pending_urls);
        }
    }
}

impl DeepLinkDispatcherState {
    fn remember_url(&mut self, url: &str) -> bool {
        let normalized = url.trim().to_string();
        let now = Instant::now();
        self.recently_received_urls
            .retain(|(_, received_at)| now.duration_since(*received_at) <= DUPLICATE_URL_WINDOW);
        if self
            .recently_received_urls
            .iter()
            .any(|(recent_url, _)| *recent_url == normalized)
        {
            return false;
        }

        self.recently_received_urls.push((normalized, now));
        true
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

pub fn remiss_urls_from_args(args: impl IntoIterator<Item = String>) -> Vec<String> {
    args.into_iter()
        .filter(|arg| arg.trim().starts_with("remiss://"))
        .collect()
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
    use std::{cell::RefCell, rc::Rc};

    use super::{parse_url, remiss_urls_from_args, DeepLinkDispatcher, DeepLinkRequest};

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

    #[test]
    fn dispatches_urls_received_before_handler_installation() {
        let dispatcher = DeepLinkDispatcher::new();
        dispatcher.receive_urls(vec!["remiss://github/rikuws/remiss/pull/17".to_string()]);

        let delivered = Rc::new(RefCell::new(Vec::new()));
        let delivered_for_handler = delivered.clone();
        dispatcher.install_handler(move |request| {
            delivered_for_handler.borrow_mut().push(request);
        });

        assert_eq!(
            delivered.borrow().as_slice(),
            &[DeepLinkRequest::GitHubPullRequest {
                repository: "rikuws/remiss".to_string(),
                number: 17,
            }]
        );
    }

    #[test]
    fn ignores_immediate_duplicate_urls_from_multiple_macos_delivery_paths() {
        let dispatcher = DeepLinkDispatcher::new();
        let delivered = Rc::new(RefCell::new(Vec::new()));
        let delivered_for_handler = delivered.clone();
        dispatcher.install_handler(move |request| {
            delivered_for_handler.borrow_mut().push(request);
        });

        dispatcher.receive_urls(vec![
            "remiss://github/rikuws/remiss/pull/17".to_string(),
            "remiss://github/rikuws/remiss/pull/17".to_string(),
        ]);

        assert_eq!(delivered.borrow().len(), 1);
    }

    #[test]
    fn extracts_remiss_urls_from_launch_args() {
        assert_eq!(
            remiss_urls_from_args([
                "-psn_0_12345".to_string(),
                "remiss://github/rikuws/remiss/pull/17".to_string(),
                "https://github.com/rikuws/remiss/pull/17".to_string(),
            ]),
            vec!["remiss://github/rikuws/remiss/pull/17".to_string()]
        );
    }
}
