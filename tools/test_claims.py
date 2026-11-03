#!/usr/bin/env python3
"""Tests for tools/claims.py, on a fake tracker. Invented logins and issues only."""

import io
import json
import os
import sys
import unittest
import urllib.error
import urllib.parse
import urllib.request
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import claims  # noqa: E402

REPO = "/repos/acme/game"
SENTINEL = "tok-SENTINEL-7f3a9c"


def lb(*names):
    return [{"name": n, "id": 100 + i} for i, n in enumerate(names)]


def cm(cid, who, body, at):
    return {"id": cid, "user": {"login": who}, "body": body, "created_at": at}


def fake_tree():
    """An invented tracker: issues #1-#5, two open PRs."""
    return {
        "user": {"login": "alice"},
        "issues": {
            1: {"number": 1, "title": "Ready server task", "state": "open", "comments": 0,
                "labels": lb("status/ready", "prio/2", "area/combat", "good-first"),
                "assignees": [], "body": "Area: combat\nRead: notes/a.md\nOwns: server/x.rs\n"},
            2: {"number": 2, "title": "Needs a client", "state": "open", "comments": 0,
                "labels": lb("status/ready", "prio/1", "needs-client"), "assignees": [],
                "body": "Owns: server/y.rs"},
            3: {"number": 3, "title": "Claimed by bob", "state": "open", "comments": 1,
                "labels": lb("status/ready", "prio/1"), "assignees": [],
                "body": "Owns: server/z.rs"},
            4: {"number": 4, "title": "Blocked", "state": "open", "comments": 0,
                "labels": lb("status/ready", "prio/1"), "assignees": [],
                "body": "Blocked by: #3\nOwns: server/w.rs"},
            5: {"number": 5, "title": "Assigned", "state": "open", "comments": 0,
                "labels": lb("status/ready", "prio/1"), "assignees": [{"login": "carol"}],
                "body": ""},
            6: {"number": 6, "title": "Top prio, free", "state": "open", "comments": 0,
                "labels": lb("status/ready", "prio/1"), "assignees": [], "body": ""},
        },
        "comments": {
            3: [cm(30, "bob", "/claim\nOwns: server/crates/eve-bot/src/, docs/status/progress.tsv",
                   "2020-01-01T00:00:00Z")],
        },
        "pulls": [
            {"number": 50, "body": "Closes #3", "user": {"login": "bob"},
             "head": {"ref": "bob/3-thing"}, "updated_at": "2020-01-02T00:00:00Z"},
            {"number": 51, "body": "unlinked", "user": {"login": "dave"},
             "head": {"ref": "dave/x"}, "updated_at": "2020-01-02T00:00:00Z"},
        ],
        "pull_files": {50: ["server/crates/eve-bot/src/a.rs"], 51: ["tools/gate.sh"]},
    }


class Fake:
    """A transport over the invented tree. Records every write."""

    has_token = True

    def __init__(self, tree, kind="github"):
        self.tree = tree
        self.writes = []
        self.kind = kind
        self.refuse = set()

    def request(self, method, path, body=None):
        u = urllib.parse.urlparse(path)
        q = urllib.parse.parse_qs(u.query)
        p = u.path
        if method != "GET":
            self.writes.append((method, p, body))
            if (method, p.rsplit("/", 1)[-1]) in self.refuse or method in self.refuse:
                raise claims.TrackerError(403, method, p)
            if method == "POST" and p.endswith("/comments"):
                n = int(p.split("/")[-2])
                lst = self.tree["comments"].setdefault(n, [])
                lst.append(cm(1000 + len(self.writes), self.tree["user"]["login"], body["body"],
                              "2099-01-01T00:00:00Z"))
                self.tree["issues"][n]["comments"] = len(lst)
            return {}
        if q.get("page", ["1"])[0] != "1":
            return []
        if p == "/user":
            return self.tree["user"]
        if p == f"{REPO}/issues":
            want = set(q["labels"][0].split(",")) if "labels" in q else set()
            out = [i for i in self.tree["issues"].values() if i["state"] == "open"
                   and want <= {x["name"] for x in i["labels"]}]
            return json.loads(json.dumps(out))
        if p.startswith(f"{REPO}/issues/") and p.endswith("/comments"):
            return list(self.tree["comments"].get(int(p.split("/")[-2]), []))
        if p.startswith(f"{REPO}/issues/"):
            return self.tree["issues"][int(p.split("/")[-1])]
        if p == f"{REPO}/pulls":
            return self.tree["pulls"]
        if p.startswith(f"{REPO}/pulls/") and p.endswith("/files"):
            return [{"filename": f} for f in self.tree["pull_files"].get(int(p.split("/")[-2]), [])]
        if p == f"{REPO}/labels":
            return [{"name": n, "id": i} for i, n in enumerate(
                ["status/ready", "status/claimed", "status/review", "needs-human"])]
        raise claims.TrackerError(404, method, p)


def tracker(tree=None, kind="github"):
    cfg = {"kind": kind, "api": "https://tracker.invalid", "repo": "acme/game"}
    fake = Fake(tree or fake_tree(), kind)
    return claims.Tracker(cfg, fake), fake


def run(argv, tr, hot=None):
    out, err = io.StringIO(), io.StringIO()
    if hot is not None:
        with mock.patch.object(claims, "hot_files", lambda: hot):
            code = claims.main(argv, tracker=tr, out=out, err=err)
    else:
        with mock.patch.object(claims, "hot_files", lambda: []):
            code = claims.main(argv, tracker=tr, out=out, err=err)
    return code, out.getvalue(), err.getvalue()


class Overlap(unittest.TestCase):
    def test_matrix(self):
        cases = [
            ("server/x.rs", "server/x.rs", True),
            ("server/", "server/x.rs", True),
            ("server", "server/a/b.rs", True),
            ("server/*.rs", "server/x.rs", True),
            ("server/x.rs", "server/*.rs", True),
            ("server/x.rs", "server/y.rs", False),
            ("server/xy", "server/x", False),
            ("notes/a.md", "tools/a.md", False),
        ]
        for a, b, want in cases:
            self.assertEqual(claims.overlaps(a, b), want, (a, b))

    def test_exempt(self):
        self.assertFalse(claims.overlaps("docs/status/progress.tsv", "docs/status/progress.tsv"))
        self.assertFalse(claims.overlaps("docs/status/x.svg", "docs/status/*.svg"))
        self.assertFalse(claims.overlaps("tools/client-ci/regress.list", "tools/client-ci/regress.list"))
        self.assertTrue(claims.overlaps("docs/status/other.tsv", "docs/status/"))


class Claims(unittest.TestCase):
    def test_earliest_claim_wins_and_release(self):
        cs = [cm(2, "bob", "/claim\nOwns: b", "2020-01-02T00:00:00Z"),
              cm(1, "alice", "/claim\nOwns: a, c/*", "2020-01-01T00:00:00Z")]
        c = claims.live_claim(cs)
        self.assertEqual((c["holder"], c["owns"]), ("alice", ["a", "c/*"]))
        cs.append(cm(3, "bob", "/release", "2020-01-03T00:00:00Z"))   # not the holder
        self.assertEqual(claims.live_claim(cs)["holder"], "alice")
        cs.append(cm(4, "alice", "/release", "2020-01-04T00:00:00Z"))
        self.assertIsNone(claims.live_claim(cs))

    def test_stale_claim_listed(self):
        tr, _ = tracker()
        code, out, _ = run(["list"], tr)
        self.assertEqual(code, 0)
        row = [ln for ln in out.splitlines() if ln.startswith("3\t")][0].split("\t")
        self.assertEqual((row[1], row[3], row[4]), ("bob", "yes", "50"))

    def test_show(self):
        tr, _ = tracker()
        self.assertEqual(run(["show", "3"], tr)[0], 0)
        self.assertEqual(run(["show", "3", "--holder-is-me"], tr)[0], 1)
        self.assertEqual(run(["show", "1"], tr)[0], 1)


class Next(unittest.TestCase):
    def test_filter_and_order(self):
        tr, fake = tracker()
        code, out, _ = run(["next"], tr)
        self.assertEqual(code, 0)
        order = [int(ln.split("\t")[0]) for ln in out.splitlines()[1:]]
        # 3 claimed, 4 blocked by open #3, 5 assigned; prio/1 before prio/2.
        self.assertEqual(order, [2, 6, 1])
        self.assertEqual(fake.writes, [])
        code, out, _ = run(["next", "--no-client"], tr)
        self.assertEqual([int(ln.split("\t")[0]) for ln in out.splitlines()[1:]], [6, 1])

    def test_unblocked_when_blocker_closed(self):
        tree = fake_tree()
        tree["issues"][3]["state"] = "closed"
        tr, _ = tracker(tree)
        out = run(["next", "--limit", "9"], tr)[1]
        self.assertIn("4\t", out)

    def test_none(self):
        tree = fake_tree()
        tree["issues"] = {}
        tr, _ = tracker(tree)
        self.assertEqual(run(["next"], tr)[0], 1)


class ClaimCmd(unittest.TestCase):
    def test_refused_when_claimed(self):
        tr, fake = tracker()
        self.assertEqual(run(["claim", "3", "--owns", "server/q.rs"], tr)[0], 4)
        self.assertEqual(fake.writes, [])

    def test_refused_on_hot_file(self):
        tr, fake = tracker()
        code, out, _ = run(["claim", "1", "--owns", "tools/land.sh"], tr, hot=["tools/land.sh"])
        self.assertEqual(code, 5)
        self.assertEqual(fake.writes, [])

    def test_hot_ok_allows(self):
        tree = fake_tree()
        tree["issues"][1]["labels"].append({"name": "hot-ok", "id": 9})
        tr, _ = tracker(tree)
        self.assertEqual(run(["claim", "1", "--owns", "tools/land.sh"], tr, hot=["tools/land.sh"])[0], 0)

    def test_overlap_with_claim_and_pr(self):
        tr, fake = tracker()
        code, out, _ = run(["check", "--owns", "server/crates/eve-bot/src/x.rs"], tr)
        self.assertEqual(code, 3)
        self.assertIn("bob", out)
        self.assertEqual(run(["check", "--owns", "tools/gate.sh"], tr)[0], 3)
        self.assertEqual(run(["check", "--owns", "docs/status/progress.tsv,server/q.rs"], tr)[0], 0)
        self.assertEqual(fake.writes, [])

    def test_claim_writes_comment_assign_label(self):
        tr, fake = tracker()
        code, out, _ = run(["claim", "1", "--owns", "server/x.rs"], tr)
        self.assertEqual(code, 0, out)
        kinds = [(m, p.rsplit("/", 1)[-1]) for m, p, _ in fake.writes]
        self.assertIn(("POST", "comments"), kinds)
        self.assertIn(("POST", "assignees"), kinds)
        self.assertIn(("POST", "labels"), kinds)
        self.assertEqual(claims.live_claim(fake.tree["comments"][1])["holder"], "alice")

    def test_github_refuses_assign_comment_stands(self):
        tr, fake = tracker()
        fake.refuse = {("POST", "assignees"), ("POST", "labels"), ("DELETE", "status%2Fready")}
        code, out, _ = run(["claim", "1", "--owns", "server/x.rs"], tr)
        self.assertEqual(code, 0, out)
        self.assertIn("comment alone is the claim", out)

    def test_lost_race_releases(self):
        tr, fake = tracker()
        orig = fake.request

        def racing(method, path, body=None):
            r = orig(method, path, body)
            if method == "POST" and path.endswith("/1/comments") and body["body"].startswith("/claim"):
                fake.tree["comments"][1].insert(0, cm(1, "bob", "/claim\nOwns: server/x.rs",
                                                      "2000-01-01T00:00:00Z"))
            return r
        fake.request = racing
        self.assertEqual(run(["claim", "1", "--owns", "server/x.rs"], tr)[0], 4)
        self.assertEqual(fake.writes[-1][2], {"body": "/release"})

    def test_no_token(self):
        tr, fake = tracker()
        fake.has_token = False
        code, _, err = run(["claim", "1", "--owns", "server/x.rs"], tr)
        self.assertEqual(code, 2)
        self.assertIn("no-token", err)


class Token(unittest.TestCase):
    def test_sentinel_never_leaks(self):
        env = {"EVE_TRACKER": "gitea", "EVE_TRACKER_API": "https://tracker.invalid/api/v1",
               "EVE_TRACKER_REPO": "acme/game", "EVE_TRACKER_TOKEN": SENTINEL}
        seen = []

        def fake_urlopen(req, timeout=None):
            seen.append((req.full_url, dict(req.header_items())))
            raise urllib.error.HTTPError(req.full_url, 401, "Unauthorized", {}, None)

        out, err = io.StringIO(), io.StringIO()
        with mock.patch.dict(os.environ, env, clear=False), \
                mock.patch.object(urllib.request, "urlopen", fake_urlopen), \
                mock.patch.object(claims, "CACHE", os.devnull):
            cfg = claims.config()
            tr = claims.Tracker(cfg, claims.Http(cfg, claims.read_token(), cache_path=os.devnull))
            code = claims.main(["whoami"], tracker=tr, out=out, err=err)
            try:
                tr.get("/user")
            except claims.TrackerError as e:
                exc_text = repr(e) + str(e) + repr(e.args)
        self.assertEqual(code, 2)
        self.assertEqual(seen[0][1].get("Authorization"), "token " + SENTINEL)
        self.assertNotIn(SENTINEL, seen[0][0])
        for text in (out.getvalue(), err.getvalue(), exc_text):
            self.assertNotIn(SENTINEL, text)
        self.assertIn("401", err.getvalue())

    def test_token_file(self):
        path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "test_claims.py")
        self.assertTrue(claims.read_token({"EVE_TRACKER_TOKEN_FILE": path}))
        self.assertIsNone(claims.read_token({}))


class HotFiles(unittest.TestCase):
    def test_parse(self):
        import tempfile
        text = ("# x\n## 9. Other\n| `nope` | x |\n## 10. Areas and hot files\n"
                "| area | docs |\n|---|---|\n| combat | `notes/re/a.md` |\n"
                "### Hot files\n| file | why |\n|---|---|\n| `tools/land.sh` | gate |\n"
                "| `server/crates/eve-server/src/service.rs` | wire |\n## 11. End\n| `no` | x |\n")
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "c.md")
            with open(p, "w", encoding="utf-8") as f:
                f.write(text)
            self.assertEqual(claims.hot_files(p),
                             ["tools/land.sh", "server/crates/eve-server/src/service.rs"])
            self.assertEqual(claims.hot_files(os.path.join(d, "absent.md")), [])


if __name__ == "__main__":
    unittest.main()
