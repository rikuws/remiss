(() => {
  const BUTTON_ID = "remiss-open-in-remiss";
  const PR_PATH_PATTERN = /^\/([^/]+)\/([^/]+)\/pull\/(\d+)(?:\/|$)/;

  let scheduled = false;
  let lastHref = window.location.href;

  function currentPullRequest() {
    const match = window.location.pathname.match(PR_PATH_PATTERN);
    if (!match) {
      return null;
    }

    return {
      owner: decodeURIComponent(match[1]),
      repo: decodeURIComponent(match[2]),
      number: match[3],
    };
  }

  function remissUrl(pullRequest) {
    const owner = encodeURIComponent(pullRequest.owner);
    const repo = encodeURIComponent(pullRequest.repo);
    return `remiss://github/${owner}/${repo}/pull/${pullRequest.number}`;
  }

  function buttonClassName() {
    if (document.querySelector(".Button")) {
      return "Button Button--primary Button--medium remiss-open-in-remiss";
    }

    return "btn btn-primary remiss-open-in-remiss";
  }

  function setButtonLabel(button, label) {
    if (!document.querySelector(".Button")) {
      button.textContent = label;
      return;
    }

    const content = document.createElement("span");
    content.className = "Button-content";

    const buttonLabel = document.createElement("span");
    buttonLabel.className = "Button-label";
    buttonLabel.textContent = label;

    content.appendChild(buttonLabel);
    button.replaceChildren(content);
  }

  function normalizedText(element) {
    return (element.textContent || element.getAttribute("aria-label") || "")
      .replace(/\s+/g, " ")
      .trim();
  }

  function isVisible(element) {
    return Boolean(element && element.getClientRects().length > 0);
  }

  function isInsidePullRequestHeader(element) {
    return Boolean(
      element.closest("header") ||
        element.closest("[class*='PullRequestHeader']") ||
        element.closest("[class*='StickyPullRequestHeader']") ||
        element.closest(".gh-header")
    );
  }

  function actionContainerFor(element) {
    return element.closest(
      [
        "[data-component='PH_Actions']",
        "[class*='PageHeader-Actions']",
        "[class*='diffStatesWrapper']",
        ".gh-header-actions",
        ".d-flex.flex-items-center.gap-2",
        ".d-flex.flex-items-center",
        ".ButtonGroup",
        ".BtnGroup",
      ].join(",")
    );
  }

  function findActionContainerByButtonLabel() {
    const labels = new Set(["View code", "Status"]);
    const controls = [...document.querySelectorAll("a, button")].filter((control) => {
      if (control.closest("footer")) {
        return false;
      }

      return (
        labels.has(normalizedText(control)) &&
        isVisible(control) &&
        isInsidePullRequestHeader(control)
      );
    });

    const preferredControl =
      controls.find((control) => normalizedText(control) === "View code") || controls[0];
    if (!preferredControl) {
      return null;
    }

    return actionContainerFor(preferredControl);
  }

  function findHeaderActions() {
    const candidates = [
      findActionContainerByButtonLabel(),
      document.querySelector(".gh-header-actions"),
      document.querySelector("[data-testid='issue-header-actions']"),
      document
        .querySelector(".js-issue-title")
        ?.closest(".gh-header")
        ?.querySelector(".gh-header-actions"),
      document.querySelector("[data-component='PH_Actions']:not(.d-none)"),
      document.querySelector("[class*='PageHeader-Actions']:not(.d-none)"),
      document.querySelector("[class*='diffStatesWrapper']"),
    ];

    return candidates.find((candidate) => candidate && isVisible(candidate)) || null;
  }

  function ensureButton() {
    const pullRequest = currentPullRequest();
    const existing = document.getElementById(BUTTON_ID);

    if (!pullRequest) {
      existing?.remove();
      return;
    }

    const targetUrl = remissUrl(pullRequest);
    const container = findHeaderActions();
    if (!container) {
      return;
    }

    if (existing?.dataset.remissTarget === targetUrl && existing.parentElement === container) {
      return;
    }

    existing?.remove();

    const button = document.createElement("a");
    button.id = BUTTON_ID;
    button.className = buttonClassName();
    button.href = targetUrl;
    button.dataset.remissTarget = targetUrl;
    button.rel = "noreferrer";
    setButtonLabel(button, "Open in Remiss");
    button.addEventListener("click", (event) => {
      event.preventDefault();
      window.location.href = targetUrl;
    });

    container.appendChild(button);
  }

  function scheduleEnsureButton() {
    if (scheduled) {
      return;
    }

    scheduled = true;
    window.requestAnimationFrame(() => {
      scheduled = false;
      if (window.location.href !== lastHref) {
        lastHref = window.location.href;
      }
      ensureButton();
    });
  }

  const observer = new MutationObserver(scheduleEnsureButton);
  observer.observe(document.documentElement, {
    childList: true,
    subtree: true,
  });

  window.addEventListener("popstate", scheduleEnsureButton);
  window.addEventListener("turbo:load", scheduleEnsureButton);
  window.addEventListener("turbo:render", scheduleEnsureButton);

  ensureButton();
})();
