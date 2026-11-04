#!/usr/bin/env python3
"""Claims on the shared issue tracker: list tasks, check paths, claim, release.

Stdlib only. Works against GitHub REST or Gitea REST (tracker-neutral). It is
read-only unless a write subcommand (`claim`, `release`, `status`) is given.

Configuration (first match wins):
  EVE_TRACKER       gitea | github
  EVE_TRACKER_API   e.g. https://api.github.com or https://gitea.example/api/v1
  EVE_TRACKER_REPO  owner/name
  otherwise derived from `git remote get-url origin` (github.com -> GitHub,
  any other host -> Gitea at https://<host>/api/v1).

The token comes only from EVE_TRACKER_TOKEN or the file named in
EVE_TRACKER_TOKEN_FILE. It is sent only in the Authorization header and never
printed. Reads work without a token on a public repo.

A claim is an issue comment whose first line is `/claim`, with a line
`Owns: p1, p2, glob/*`. The earliest live `/claim` wins; a `/release` by the
holder ends it. Where the token may, the tool also assigns the holder and sets
the label `status/claimed`; on GitHub without triage rights the comment alone
is the claim.

Exit codes: 0 ok; 1 nothing (no task, not claimed, not mine); 2 tracker error
or no token; 3 path overlap; 4 someone else holds it (or it is closed,
assigned or needs-owner); 5 a hot file without the `hot-ok` label.
"""

import argparse
import datetime as dt
import fnmatch
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CACHE = os.path.join(ROOT, "out", "tracker-cache.json")
CACHE_TTL = 60
CONTRIBUTING = os.path.join(ROOT, "docs", "guides", "contributing.md")

# Append-only files: two claims may both name them.
EXEMPT = [
    "docs/status/progress.tsv",
    "docs/status/*.svg",
    "docs/status/README.md",
    "docs/status/waves.tsv",
    "tools/client-ci/regress.list",
    "docs/packets.tsv",
]
DROP_LABELS = {"status/claimed", "needs-owner", "needs-human", "parked", "status/blocked"}
STALE_DAYS = 7


class TrackerError(Exception):
    """A tracker call failed. Carries the HTTP status, method and path only."""

    def __init__(self, status, method, path):
        super().__init__(f"tracker error: {status} {method} {path}")
        self.status = status


class NoToken(Exception):
    pass


# ---------------------------------------------------------------- config


def git(*args):
    try:
        return subprocess.run(["git", "-C", ROOT, *args], capture_output=True, text=True,
                              timeout=10).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return ""


def config(env=None):
    env = os.environ if env is None else env
    kind, api, repo = env.get("EVE_TRACKER"), env.get("EVE_TRACKER_API"), env.get("EVE_TRACKER_REPO")
    if not (kind and api and repo):
        url = git("remote", "get-url", "origin")
        m = re.match(r"(?:\w+://)?(?:[^@/]+@)?([^/:]+)(?::\d+)?[:/](.+?)(?:\.git)?/?$", url)
        if m:
            host, path = m.group(1), m.group(2)
            gh = host == "github.com"
            kind = kind or ("github" if gh else "gitea")
            api = api or ("https://api.github.com" if gh else f"https://{host}/api/v1")
            repo = repo or "/".join(path.split("/")[-2:])
    if kind not in ("github", "gitea") or not api or not repo:
        raise TrackerError("config", "-", "set EVE_TRACKER, EVE_TRACKER_API and EVE_TRACKER_REPO")
    return {"kind": kind, "api": api.rstrip("/"), "repo": repo}


def read_token(env=None):
    env = os.environ if env is None else env
    tok = env.get("EVE_TRACKER_TOKEN", "").strip()
    if not tok and env.get("EVE_TRACKER_TOKEN_FILE"):
        try:
            with open(env["EVE_TRACKER_TOKEN_FILE"], encoding="utf-8") as f:
                tok = f.read().strip()
        except OSError:
            tok = ""
    return tok or None


# ---------------------------------------------------------------- transport


class Http:
    """urllib transport. The token lives only in this object and the header."""

    def __init__(self, cfg, token, cache_path=CACHE):
        self.cfg = cfg
        self._token = token
        self.cache_path = cache_path
        self.has_token = bool(token)

    def _cache(self):
        try:
            with open(self.cache_path, encoding="utf-8") as f:
                return json.load(f)
        except (OSError, ValueError):
            return {}

    def _store(self, data):
        try:
            os.makedirs(os.path.dirname(self.cache_path), exist_ok=True)
            with open(self.cache_path, "w", encoding="utf-8") as f:
                json.dump(data, f)
        except OSError:
            pass

    def request(self, method, path, body=None):
        url = self.cfg["api"] + path
        key = ("auth " if self._token else "anon ") + url
        if method == "GET":
            c = self._cache().get(key)
            if c and time.time() - c["t"] < CACHE_TTL:
                return c["v"]
        headers = {"Accept": "application/json", "User-Agent": "eve-offline-claims"}
        if self._token:
            scheme = "Bearer" if self.cfg["kind"] == "github" else "token"
            headers["Authorization"] = f"{scheme} {self._token}"
        data = None
        if body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        req = urllib.request.Request(url, data=data, headers=headers, method=method)
        status = None
        try:
            with urllib.request.urlopen(req, timeout=15) as r:
                raw = r.read()
        except urllib.error.HTTPError as e:
            status = e.code
        except (urllib.error.URLError, OSError, ValueError):
            status = "unreachable"
        if status is not None:
            raise TrackerError(status, method, path) from None
        val = json.loads(raw) if raw.strip() else None
        cache = self._cache() if method == "GET" else {}
        if method == "GET":
            cache[key] = {"t": time.time(), "v": val}
        self._store(cache)  # a write empties the cache
        return val


# ---------------------------------------------------------------- tracker


class Tracker:
    def __init__(self, cfg, transport):
        self.cfg = cfg
        self.t = transport
        self.gh = cfg["kind"] == "github"
        self.r = f"/repos/{cfg['repo']}"
        self._label_ids = None
        self._me = None

    def get(self, path):
        return self.t.request("GET", path)

    def me(self):
        if self._me is None:
            if not getattr(self.t, "has_token", True):
                raise NoToken()
            self._me = self.get("/user")["login"]
        return self._me

    def paged(self, path):
        out, page = [], 1
        sep = "&" if "?" in path else "?"
        size = "per_page=100" if self.gh else "limit=50"
        while True:
            items = self.get(f"{path}{sep}page={page}&{size}") or []
            out.extend(items)
            if len(items) < (100 if self.gh else 50):
                return out
            page += 1

    def open_issues(self, labels=()):
        q = "state=open" + ("" if self.gh else "&type=issues")
        if labels:
            q += "&labels=" + urllib.parse.quote(",".join(labels))
        return [i for i in self.paged(f"{self.r}/issues?{q}") if "pull_request" not in i
                or i.get("pull_request") is None]

    def issue(self, n):
        return self.get(f"{self.r}/issues/{n}")

    def comments(self, n):
        return self.paged(f"{self.r}/issues/{n}/comments")

    def open_pulls(self):
        return self.paged(f"{self.r}/pulls?state=open")

    def pull_files(self, n):
        return [f.get("filename", "") for f in self.paged(f"{self.r}/pulls/{n}/files")]

    # writes
    def comment(self, n, body):
        self.t.request("POST", f"{self.r}/issues/{n}/comments", {"body": body})

    def assign(self, n, add):
        me = self.me()
        if self.gh:
            m = "POST" if add else "DELETE"
            self.t.request(m, f"{self.r}/issues/{n}/assignees", {"assignees": [me]})
        else:
            cur = [a["login"] for a in (self.issue(n).get("assignees") or [])]
            new = sorted(set(cur) | {me}) if add else [a for a in cur if a != me]
            self.t.request("PATCH", f"{self.r}/issues/{n}", {"assignees": new})

    def label_id(self, name):
        if self._label_ids is None:
            self._label_ids = {lb["name"]: lb["id"] for lb in self.paged(f"{self.r}/labels")}
        if name not in self._label_ids:
            raise TrackerError("no-label", "GET", f"{self.r}/labels ({name})")
        return self._label_ids[name]

    def add_label(self, n, name):
        val = name if self.gh else self.label_id(name)
        self.t.request("POST", f"{self.r}/issues/{n}/labels", {"labels": [val]})

    def remove_label(self, n, name):
        val = urllib.parse.quote(name, safe="") if self.gh else self.label_id(name)
        self.t.request("DELETE", f"{self.r}/issues/{n}/labels/{val}")


# ---------------------------------------------------------------- parsing


def labels_of(issue):
    return {lb["name"] if isinstance(lb, dict) else lb for lb in issue.get("labels") or []}


def assignees_of(issue):
    return [a["login"] for a in issue.get("assignees") or []]


def body_field(issue, name):
    m = re.search(rf"^\s*{name}:\s*(.+?)\s*$", issue.get("body") or "", re.M | re.I)
    return m.group(1) if m else ""


def split_paths(s):
    return [p.strip().strip("`") for p in re.split(r"[,\s]+", s or "") if p.strip().strip("`")]


def parse_time(s):
    try:
        return dt.datetime.fromisoformat((s or "").replace("Z", "+00:00"))
    except ValueError:
        return None


def live_claim(comments):
    """The earliest live /claim: {holder, owns, at} or None."""
    holder = None
    for c in sorted(comments, key=lambda c: (c.get("created_at") or "", c.get("id") or 0)):
        text = (c.get("body") or "").strip()
        first = text.split("\n", 1)[0].strip()
        who = (c.get("user") or {}).get("login")
        if first == "/claim" and holder is None:
            m = re.search(r"^\s*Owns:\s*(.+)$", text, re.M)
            holder = {"holder": who, "owns": split_paths(m.group(1) if m else ""),
                      "at": c.get("created_at")}
        elif first == "/release" and holder is not None and who == holder["holder"]:
            holder = None
    return holder


def exempt(p):
    return any(p == e or fnmatch.fnmatch(p, e) for e in EXEMPT)


def overlaps(a, b):
    """Two path patterns overlap: either matches the other, or one is a dir prefix."""
    if exempt(a) or exempt(b):
        return False
    a2, b2 = a.rstrip("/"), b.rstrip("/")
    if a2 == b2 or fnmatch.fnmatch(a2, b2) or fnmatch.fnmatch(b2, a2):
        return True
    return b2.startswith(a2 + "/") or a2.startswith(b2 + "/")


SERVER_SRC = "server/crates/eve-server/"


def hot_files(path=CONTRIBUTING):
    """Every backticked path in the first cell of the hot-files table of `## 10.`.

    The table follows a `### ...hot...` heading or a `**Hot files.**` paragraph; the area
    table before it is not read, and reading stops where the table ends. Its `src/...`
    paths are under `server/crates/eve-server/` (the doc says so), so they are returned as
    repo paths, the form claims use. The "registration tables" row names no path in its
    first cell on purpose: only one line of those files is maintainer-only, not the file.
    None when the doc or the table cannot be found, so a claim fails closed.
    """
    try:
        with open(path, encoding="utf-8") as f:
            text = f.read()
    except OSError:
        return None
    m = re.search(r"^## 10\.[^\n]*$(.*?)(?=^## |\Z)", text, re.M | re.S)
    if not m:
        return None
    h = re.search(r"^(?:#{3,}[^\n]*\bhot\b|\*\*hot files)[^\n]*$(.*?)(?=^#{2,} |\Z)",
                  m.group(1), re.M | re.S | re.I)
    if not h:
        return None
    out, started = [], False
    for line in h.group(1).split("\n"):
        if not line.startswith("|"):
            if started:
                break
            continue
        started = True
        first = line.strip("|").split("|", 1)[0]
        for t in re.findall(r"`([^`]+)`", first):
            out.append(SERVER_SRC + t if t.startswith("src/") else t)
    return out if started else None


def pr_issue_refs(pr):
    return {int(x) for x in re.findall(
        r"(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s+#(\d+)", pr.get("body") or "", re.I)}


def now():
    return dt.datetime.now(dt.timezone.utc)


# ---------------------------------------------------------------- commands


def all_claims(tr):
    """{issue number: claim} for every open issue with a live /claim."""
    out = {}
    for i in tr.open_issues():
        if not i.get("comments"):
            continue
        c = live_claim(tr.comments(i["number"]))
        if c:
            c["issue"] = i
            out[i["number"]] = c
    return out


def cmd_whoami(tr, a, out):
    print(tr.me(), file=out)
    return 0


def cmd_list(tr, a, out):
    pulls = tr.open_pulls()
    print("issue\tholder\tage_days\tstale\tpr\towns", file=out)
    for n, c in sorted(all_claims(tr).items()):
        at = parse_time(c["at"])
        age = (now() - at).days if at else 0
        pr = next((p for p in pulls if n in pr_issue_refs(p)), None)
        upd = parse_time(pr.get("updated_at")) if pr else None
        stale = age > STALE_DAYS and (upd is None or (now() - upd).days > STALE_DAYS)
        print(f"{n}\t{c['holder']}\t{age}\t{'yes' if stale else 'no'}\t"
              f"{pr['number'] if pr else 'none'}\t{','.join(c['owns'])}", file=out)
    return 0


def cmd_show(tr, a, out):
    c = live_claim(tr.comments(a.n))
    if not c:
        print(f"#{a.n}: unclaimed", file=out)
        return 1
    pr = next((p["number"] for p in tr.open_pulls() if a.n in pr_issue_refs(p)), "none")
    print(f"issue={a.n}\nholder={c['holder']}\nowns={','.join(c['owns'])}\npr={pr}", file=out)
    if a.holder_is_me and c["holder"] != tr.me():
        return 1
    return 0


def find_overlaps(tr, owns, issue=None):
    clashes = []
    for n, c in all_claims(tr).items():
        if n == issue:
            continue
        for p in owns:
            for q in c["owns"]:
                if overlaps(p, q):
                    clashes.append((n, c["holder"], p, q))
    for pr in tr.open_pulls():
        if issue is not None and issue in pr_issue_refs(pr):
            continue
        who = (pr.get("user") or {}).get("login")
        for f in tr.pull_files(pr["number"]):
            for p in owns:
                if overlaps(p, f):
                    clashes.append((f"PR {pr['number']}", who, p, f))
    return clashes


def cmd_check(tr, a, out):
    clashes = find_overlaps(tr, split_paths(a.owns), a.issue)
    for where, who, p, q in clashes:
        print(f"overlap\t{where}\t{who}\t{p}\t{q}", file=out)
    return 3 if clashes else 0


def cmd_next(tr, a, out):
    rows = []
    open_cache = {}

    def is_open(n):
        if n not in open_cache:
            try:
                open_cache[n] = tr.issue(n).get("state") == "open"
            except TrackerError:
                open_cache[n] = False
        return open_cache[n]

    for i in tr.open_issues(["status/ready"]):
        labels = labels_of(i)
        if assignees_of(i) or labels & DROP_LABELS:
            continue
        if a.no_client and "needs-client" in labels:
            continue
        area = next((x[5:] for x in labels if x.startswith("area/")), "") or body_field(i, "Area")
        if a.area and area != a.area:
            continue
        blocked = [int(x) for x in re.findall(r"#(\d+)", body_field(i, "Blocked by"))]
        if any(is_open(b) for b in blocked):
            continue
        if i.get("comments") and live_claim(tr.comments(i["number"])):
            continue
        prio = next((int(x[5:]) for x in sorted(labels) if re.fullmatch(r"prio/[123]", x)), 9)
        due = ((i.get("milestone") or {}).get("due_on")) or "9999"
        first = 0 if (a.first and "good-first" in labels) else 1
        rows.append(((prio, due, first, i["number"]), i, area, labels))
    rows.sort(key=lambda r: r[0])
    if a.limit:
        rows = rows[:a.limit]
    if not rows:
        print("no task: nothing open, ready, unclaimed and unblocked", file=out)
        return 1
    print("issue\tprio\tarea\tclient\ttitle\tread\towns", file=out)
    for key, i, area, labels in rows:
        p = f"prio/{key[0]}" if key[0] < 9 else "-"
        cl = "yes" if "needs-client" in labels else "no"
        print(f"{i['number']}\t{p}\t{area or '-'}\t{cl}\t{i.get('title', '')}\t"
              f"{body_field(i, 'Read') or '-'}\t{body_field(i, 'Owns') or '-'}", file=out)
    return 0


def soft(tr, out, what, fn, *args):
    """A write that GitHub may refuse without triage rights; the comment stands."""
    try:
        fn(*args)
    except TrackerError as e:
        if tr.gh and e.status in (403, 404, 422):
            print(f"note: could not {what} ({e.status}); the /claim comment alone is the claim",
                  file=out)
        else:
            raise


def cmd_claim(tr, a, out, hot=None):
    owns = split_paths(a.owns)
    if not owns:
        print("claim needs --owns", file=out)
        return 3
    me = tr.me()
    i = tr.issue(a.n)
    labels = labels_of(i)
    held = live_claim(tr.comments(a.n)) if i.get("comments") else None
    others = [x for x in assignees_of(i) if x != me]
    if (i.get("state") != "open" or others or "needs-owner" in labels
            or (held and held["holder"] != me)
            or ("status/claimed" in labels and not (held and held["holder"] == me))):
        print(f"#{a.n}: closed, assigned, claimed or needs-owner; pick another", file=out)
        return 4
    hot = hot_files() if hot is None else hot
    if hot is None:
        print("could not read the hot-file table in docs/guides/contributing.md §10; "
              "a maintainer must fix the doc or tools/claims.py", file=out)
        return 5
    hit = [(p, h) for p in owns for h in hot if overlaps(p, h)]
    if hit and "hot-ok" not in labels:
        for p, h in hit:
            print(f"hot\t{p}\t{h}", file=out)
        print("a hot file needs the label hot-ok from a maintainer", file=out)
        return 5
    clashes = find_overlaps(tr, owns, a.n)
    if clashes:
        for where, who, p, q in clashes:
            print(f"overlap\t{where}\t{who}\t{p}\t{q}", file=out)
        return 3
    if not held:
        tr.comment(a.n, "/claim\nOwns: " + ", ".join(owns))
    soft(tr, out, "assign", tr.assign, a.n, True)
    if "status/ready" in labels:
        soft(tr, out, "remove status/ready", tr.remove_label, a.n, "status/ready")
    soft(tr, out, "label status/claimed", tr.add_label, a.n, "status/claimed")
    after = live_claim(tr.comments(a.n))
    if not after or after["holder"] != me:
        tr.comment(a.n, "/release")
        print(f"#{a.n}: someone claimed first; released", file=out)
        return 4
    print(f"claimed #{a.n} as {me}; owns {', '.join(owns)}", file=out)
    return 0


def cmd_release(tr, a, out):
    tr.comment(a.n, "/release")
    soft(tr, out, "unassign", tr.assign, a.n, False)
    i = tr.issue(a.n)
    labels = labels_of(i)
    if "status/claimed" in labels:
        soft(tr, out, "remove status/claimed", tr.remove_label, a.n, "status/claimed")
    if i.get("state") == "open" and "status/ready" not in labels:
        soft(tr, out, "label status/ready", tr.add_label, a.n, "status/ready")
    print(f"released #{a.n}", file=out)
    return 0


def cmd_status(tr, a, out):
    labels = labels_of(tr.issue(a.n))
    if a.state == "needs-human":
        soft(tr, out, "label needs-human", tr.add_label, a.n, "needs-human")
    else:
        want = f"status/{a.state}"
        for lb in sorted(labels):
            if lb.startswith("status/") and lb != want:
                soft(tr, out, f"remove {lb}", tr.remove_label, a.n, lb)
        if want not in labels:
            soft(tr, out, f"label {want}", tr.add_label, a.n, want)
    if a.why:
        tr.comment(a.n, f"{a.state}: {a.why}")
    print(f"#{a.n}: {a.state}", file=out)
    return 0


def cmd_pr_for_branch(tr, a, out):
    branch = a.branch or git("rev-parse", "--abbrev-ref", "HEAD")
    for p in tr.open_pulls():
        if (p.get("head") or {}).get("ref") == branch:
            print(p["number"], file=out)
            return 0
    print("none", file=out)
    return 1


def parser():
    ap = argparse.ArgumentParser(prog="claims.py", description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("whoami")
    sub.add_parser("list")
    s = sub.add_parser("show")
    s.add_argument("n", type=int)
    s.add_argument("--holder-is-me", action="store_true")
    s = sub.add_parser("check")
    s.add_argument("--owns", required=True)
    s.add_argument("--issue", type=int)
    s = sub.add_parser("next")
    s.add_argument("--no-client", action="store_true")
    s.add_argument("--area")
    s.add_argument("--first", action="store_true")
    s.add_argument("--limit", type=int, default=5)
    s = sub.add_parser("claim")
    s.add_argument("n", type=int)
    s.add_argument("--owns", required=True)
    s = sub.add_parser("release")
    s.add_argument("n", type=int)
    s = sub.add_parser("status")
    s.add_argument("n", type=int)
    s.add_argument("state", choices=["review", "needs-human", "blocked"])
    s.add_argument("--why")
    s = sub.add_parser("pr-for-branch")
    s.add_argument("--branch")
    return ap


COMMANDS = {
    "whoami": cmd_whoami, "list": cmd_list, "show": cmd_show, "check": cmd_check,
    "next": cmd_next, "claim": cmd_claim, "release": cmd_release, "status": cmd_status,
    "pr-for-branch": cmd_pr_for_branch,
}


def main(argv=None, tracker=None, out=None, err=None):
    out = out or sys.stdout
    err = err or sys.stderr
    a = parser().parse_args(argv)
    try:
        if tracker is None:
            cfg = config()
            tracker = Tracker(cfg, Http(cfg, read_token()))
        return COMMANDS[a.cmd](tracker, a, out)
    except NoToken:
        print("no-token: set EVE_TRACKER_TOKEN or EVE_TRACKER_TOKEN_FILE "
              "(or ask your human to run this command)", file=err)
        return 2
    except TrackerError as e:
        print(str(e), file=err)
        return 2


if __name__ == "__main__":
    sys.exit(main())
