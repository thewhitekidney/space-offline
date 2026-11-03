#!/usr/bin/env python3
"""Tests for tools/port_surface.py and tools/port-surface.sh (PORT-SURFACE, T2).

Every tree here is invented: made-up services, events and ids, no client bytes.
The shell wrapper runs against a fake `docker` that maps the container's paths
(/work, /o, /tree) onto temporary directories and writes tiny tables, so nothing
here needs Docker, a client or the owner's prefix.
"""
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import port_surface as ps  # noqa: E402

CALLS_HEAD = 'service\tmethod\tcall_site\tkind\tconsumer\tguard\tserved\tnotes\n'
PUSH_HEAD = 'event\thandler_file\tservice\tdeclared_in\tarity\tparams\tpayload_shape\trisk\tnotes\n'
SESSION_HEAD = 'key\ttype\torigin\tset_by\treaders\tabsent\twrong\tconfidence\tcitation\n'
ERR_HEAD = ('key\tsource\tdialog_id\tdialog_type\tpresentation\tsuppressable\tclosable\ttitle_id\tbody_id\t'
            'loc_args\traise_sites\traised_at\targs_at_raise\targs_read_by_handler\tcaught_by_key_at\t'
            'caught_in\tcaught_via\thandler_fn_refs\tui_behaviour\n')
CALLABLE_HEAD = 'service\tclass\tguid\tmodule\tdecl_line\tmethod\tmethod_line\targs\tmetadata\treturns\n'


def rd(path):
    with open(path) as f:
        return f.read()


def put(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, 'w') as f:
        f.write(text)


def call(svc, meth, n=2, kind='call', consumer='use', line=10, served='yes'):
    return '%s\t%s\tinvented/a.py:%d\t%s\t%s\tnone\t%s\tin f(), %d args\n' % (svc, meth, line, kind, consumer, served, n)


def push(ev, path='invented/ui.py', line=5, params='a, b', shape='pass', arity='2', svc='Svc [svc.x]'):
    return '%s\t%s:%d\t%s\tnotifyevents\t%s\t%s\t%s\tnone\tn\n' % (ev, path, line, svc, arity, params, shape)


def session(key, reads, mods=3):
    return '%s\tint\treceived\tx\t%d reads in %d modules; prose\ta\tw\tV\tinvented/g.py:1\n' % (key, reads, mods)


def err(key, src='fsd+loc+pyc', loc='n', raise_sites='1', via='msg'):
    return ('%s\t%s\t1\tinfo\tmodal\tTrue\tTrue\t2\t3\t%s\t%s\tinvented/a.py:3\t-\t-\tinvented/b.py:4\tf\t%s\t-\t-\n'
            % (key, src, loc, raise_sites, via))


class Tree:
    """One side's output directory."""

    def __init__(self, root, name):
        self.dir = os.path.join(root, name)
        os.makedirs(self.dir)

    def w(self, name, text):
        put(os.path.join(self.dir, name), text)
        return self


class Compare(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix='test-port-surface-')
        self.repo = os.path.join(self.tmp, 'repo')
        put(self.repo + '/server/crates/eve-server/src/service.rs',
            'fn f() { send("OnInventedEvent"); match m { "InventedErr" => 1, "ZetaMsg" => 2 } let _ = "GetThing"; '
            'let _ = "eve_public.a.ZetaMsg"; const P: &str = "eve_public.pk.sub."; }\n')
        self.old = Tree(self.tmp, 'old')
        self.new = Tree(self.tmp, 'new')
        self.out = os.path.join(self.tmp, 'out')
        os.makedirs(self.out)
        put(self.out + '/served.json', json.dumps({'services': ['alpha', 'beta::sub'], 'pairs': [
            'alpha\tGetThing', 'alpha\tSetThing', 'beta::sub\tPing']}))

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def both(self, name, a, b):
        self.old.w(name, a)
        self.new.w(name, b)

    def run_cmp(self):
        rep = ps.run(self.old.dir, self.new.dir, self.out, self.repo)
        rows = {(r[0], r[1], r[2]): r for r in rep.rows}
        return rep, rows

    def test_identical_trees_have_no_rows_and_exit_zero(self):
        t = CALLS_HEAD + call('alpha', 'GetThing')
        self.both('client-calls.tsv', t, t)
        self.both('push-events.tsv', PUSH_HEAD + push('OnInventedEvent'), PUSH_HEAD + push('OnInventedEvent'))
        rep, rows = self.run_cmp()
        self.assertEqual(rep.rows, [])
        self.assertEqual(ps.main([self.old.dir, self.new.dir, '--out', self.out, '--repo', self.repo]), 0)
        md = rd(self.out + '/surface.md')
        self.assertIn('no served impact (exit 0)', md)

    def test_calls_removed_arity_and_added(self):
        self.both('client-calls.tsv',
                  CALLS_HEAD + call('alpha', 'GetThing', 2) + call('alpha', 'SetThing', 1) + call('gamma', 'Old', 1, served='no'),
                  CALLS_HEAD + call('alpha', 'GetThing', 3, line=99) + call('delta', 'New', 1, served='no') +
                  call('alpha', 'SetThing', 1, line=50))
        rep, rows = self.run_cmp()
        self.assertTrue(rows[('G1', 'arity', 'alpha.GetThing')][5], 'served pair whose arity changed is an impact')
        self.assertEqual(rows[('G1', 'arity', 'alpha.GetThing')][3], 'args [2] -> [3]')
        self.assertFalse(rows[('G1', 'removed', 'gamma.Old')][5], 'a call we do not answer is not an impact')
        self.assertFalse(rows[('G1', 'added', 'delta.New')][5])
        self.assertNotIn(('G1', 'arity', 'alpha.SetThing'), rows, 'a moved line is not a change')
        self.assertEqual(len(rep.impacts()), 1)

    def test_served_state_comes_from_the_dispatcher_not_the_table(self):
        # the table says 'no', the dispatcher answers it: the dispatcher wins
        self.both('client-calls.tsv', CALLS_HEAD + call('alpha', 'GetThing', 1, served='no'), CALLS_HEAD)
        rep, rows = self.run_cmp()
        r = rows[('G1', 'removed', 'alpha.GetThing')]
        self.assertEqual(r[4], 'yes')
        self.assertTrue(r[5])
        # a bound sub-object name `beta::sub` answers for the client's `beta`
        self.both('client-calls.tsv', CALLS_HEAD + call('beta', 'Ping', 1, served='no'), CALLS_HEAD)
        rep, rows = self.run_cmp()
        self.assertTrue(rows[('G1', 'removed', 'beta.Ping')][5])

    def test_consumer_change_is_reported_but_is_not_an_impact(self):
        self.both('client-calls.tsv', CALLS_HEAD + call('alpha', 'GetThing', consumer='use'),
                  CALLS_HEAD + call('alpha', 'GetThing', consumer='native'))
        rep, rows = self.run_cmp()
        self.assertIn(('G1', 'consumer', 'alpha.GetThing'), rows)
        self.assertEqual(rep.impacts(), [])

    def test_push_signature_shape_and_served_events(self):
        self.both('push-events.tsv',
                  PUSH_HEAD + push('OnInventedEvent', params='a') + push('OnOther', params='a') + push('OnMoved', line=3),
                  PUSH_HEAD + push('OnInventedEvent', params='a, b', arity='3') + push('OnOther', params='a', shape='scatter')
                  + push('OnMoved', line=400))
        rep, rows = self.run_cmp()
        self.assertTrue(rows[('G3', 'signature', 'OnInventedEvent @ invented/ui.py')][5])
        shape = rows[('G3', 'shape', 'OnOther @ invented/ui.py')]
        self.assertFalse(shape[5], 'not sent by our crates')
        self.assertNotIn(('G3', 'signature', 'OnMoved @ invented/ui.py'), rows)

    def test_push_duplicates_are_compared_as_a_multiset(self):
        a = PUSH_HEAD + push('OnDup', line=1) + push('OnDup', line=2)
        b = PUSH_HEAD + push('OnDup', line=1)
        self.both('push-events.tsv', a, b)
        rep, rows = self.run_cmp()
        self.assertIn(('G3', 'signature', 'OnDup @ invented/ui.py'), rows)

    def test_push_event_with_no_handler_left(self):
        self.both('push-events.tsv', PUSH_HEAD + push('OnInventedEvent'), PUSH_HEAD + push('OnElse'))
        rep, rows = self.run_cmp()
        self.assertTrue(rows[('G3', 'event-gone', 'OnInventedEvent')][5])
        self.assertTrue(rows[('G3', 'removed', 'OnInventedEvent @ invented/ui.py')][5])

    def test_session_keys_reads_and_gone(self):
        self.both('session-keys.tsv',
                  SESSION_HEAD + session('alpha', 10) + session('beta', 4) + session('gamma', 1),
                  SESSION_HEAD + session('alpha', 11) + session('beta', 0) + session('delta', 2))
        rep, rows = self.run_cmp()
        self.assertEqual(rows[('G2', 'readers', 'alpha')][3], '10 reads in 3 modules -> 11 reads in 3 modules')
        self.assertFalse(rows[('G2', 'readers', 'alpha')][5])
        self.assertTrue(rows[('G2', 'readers', 'beta')][5], 'read before, never now')
        self.assertTrue(rows[('G2', 'removed', 'gamma')][5])
        self.assertIn(('G2', 'added', 'delta'), rows)

    def test_session_unlisted_attributes_are_only_reported_when_new(self):
        self.both('session-keys.tsv', SESSION_HEAD + session('alpha', 1), SESSION_HEAD + session('alpha', 1))
        self.old.w('session-keys.log', 'session-keys: 1 keys, 0 drifted; most-read session attributes not in the table: foo(9), bar(3)\n')
        self.new.w('session-keys.log', 'session-keys: 1 keys, 0 drifted; most-read session attributes not in the table: foo(9), baz(2)\n')
        rep, rows = self.run_cmp()
        self.assertIn(('G2', 'unlisted', 'baz'), rows)
        self.assertNotIn(('G2', 'unlisted', 'foo'), rows)

    GW_OLD = ('FILE eve_public/a.proto pkg eve_public.a\n'
              'message ZetaMsg\n  1 string name\nmessage Quiet\n  1 string x\n'
              'message Gone\n  1 string y\nrpc Ping(.x)->.y\n'
              'FILE eve_public/pk/s.proto pkg eve_public.pk.sub.api\nmessage Built\n  1 string a\n')
    GW_NEW = ('FILE eve_public/a.proto pkg eve_public.a\n'
              'message ZetaMsg\n  1 string name\n  2 int64 id\nmessage Quiet\n  1 int32 x\n'
              'message Fresh\n  1 string z\nrpc Ping(.x)->.y\n'
              'FILE eve_public/pk/s.proto pkg eve_public.pk.sub.api\nmessage Built\n  1 int32 a\n')

    def test_gateway_message_changes_and_the_crate_search(self):
        self.both('gw-protos.txt', self.GW_OLD, self.GW_NEW)
        rep, rows = self.run_cmp()
        z = rows[('G4', 'changed', 'eve_public/a.proto message ZetaMsg')]
        self.assertTrue(z[5], 'our crates name ZetaMsg')
        self.assertIn('2 int64 id', z[3])
        self.assertEqual(z[4], 'name')
        self.assertFalse(rows[('G4', 'changed', 'eve_public/a.proto message Quiet')][5],
                         'a package that merely contains a named message is not a claim on its other messages')
        b = rows[('G4', 'changed', 'eve_public/pk/s.proto message Built')]
        self.assertTrue(b[5])
        self.assertEqual(b[4], 'package')
        self.assertFalse(rows[('G4', 'removed', 'eve_public/a.proto message Gone')][5])
        self.assertIn(('G4', 'added', 'eve_public/a.proto message Fresh'), rows)
        self.assertNotIn(('G4', 'changed', 'eve_public/a.proto rpc Ping(.x)->.y'), rows)

    def test_nested_message_change_belongs_to_its_top_level_message(self):
        a = 'FILE f.proto pkg p\nmessage Outer\n  1 string a\n  message Inner\n    1 string b\n'
        b = 'FILE f.proto pkg p\nmessage Outer\n  1 string a\n  message Inner\n    1 int32 b\n'
        self.both('gw-protos.txt', a, b)
        rep, rows = self.run_cmp()
        self.assertEqual([k for k in rows if k[0] == 'G4'], [('G4', 'changed', 'f.proto message Outer')])

    def test_services_remote_bound_and_callable(self):
        self.both('remote-services.txt',
                  '=== remote services referenced by client (3) ===\n'
                  '  alpha                        refs=  97  methods(3): A, B, C\n'
                  '  gamma                        refs=   5  methods(1): X\n'
                  '  omega                        refs=   2  methods(1): Y\n',
                  '=== remote services referenced by client (3) ===\n'
                  '  alpha                        refs=  98  methods(4): A, B, C, D\n'
                  '  omega                        refs=   2  methods(1): Y\n'
                  '  sigma                        refs=   1  methods(1): Z\n')
        bound_old = {'services': {'alpha': {'GetThing': {'arities': ['2']}, 'SetThing': {'arities': ['1']}}, 'zed': {'M': {'arities': ['0']}}}}
        bound_new = {'services': {'alpha': {'GetThing': {'arities': ['3']}}, 'zed': {'M': {'arities': ['0']}}}}
        self.both('bound-objects.json', json.dumps(bound_old), json.dumps(bound_new))
        self.both('client-services.json',
                  json.dumps({'S1': {'module': 'm', 'class': 'C', 'guid': 'g', 'methods': ['a', 'b']}}),
                  json.dumps({'S1': {'module': 'm', 'class': 'C', 'guid': 'g2', 'methods': ['a']}}))
        self.both('client-callable.tsv',
                  CALLABLE_HEAD + 'svc\tK\tg\tm\t1\tGetThing\t2\tx\t[]\tNone\n' + 'svc\tK\tg\tm\t1\tOther\t2\tx\t[]\tNone\n',
                  CALLABLE_HEAD + 'svc\tK\tg\tm\t1\tOther\t2\tx\t[]\tNone\n')
        rep, rows = self.run_cmp()
        self.assertFalse(rows[('G5', 'remote-removed', 'gamma')][5])
        self.assertIn(('G5', 'remote-added', 'sigma'), rows)
        self.assertEqual(rows[('G5', 'remote-methods', 'alpha')][3], 'method count 3 -> 4 (refs 97 -> 98)')
        self.assertTrue(rows[('G5', 'bound-methods', 'alpha')][5], 'GetThing arity and SetThing are ours')
        self.assertFalse(rows[('G5', 'bound-methods', 'alpha')][3] == '')
        self.assertNotIn(('G5', 'bound-methods', 'zed'), rows)
        sc = rows[('G5', 'service-changed', 'S1')]
        self.assertIn('guid g -> g2', sc[3])
        self.assertIn("methods removed ['b']", sc[3])
        self.assertTrue(rows[('G5', 'callable-removed', 'svc.GetThing')][5], 'our crates name GetThing')

    def test_user_errors_loc_args_and_missing_resources(self):
        self.both('user-errors.tsv',
                  ERR_HEAD + err('InventedErr', loc='a') + err('Unused', loc='a') + err('Gone'),
                  ERR_HEAD + err('InventedErr', loc='a, b') + err('Unused', loc='z') + err('Fresh'))
        rep, rows = self.run_cmp()
        r = rows[('G6', 'changed', 'InventedErr')]
        self.assertTrue(r[5])
        self.assertIn('loc_args a -> a, b', r[3])
        self.assertFalse(rows[('G6', 'changed', 'Unused')][5], 'not named by our crates')
        self.assertFalse(rows[('G6', 'removed', 'Gone')][5])
        # a side read without an install: resource columns are not compared
        self.old.w('meta.txt', 'resources=no\n')
        rep, rows = self.run_cmp()
        self.assertNotIn(('G6', 'changed', 'InventedErr'), rows)
        self.assertTrue(any('only the bytecode columns' in n for n in rep.notes['G6']))

    def test_a_missing_table_is_a_note_not_a_crash(self):
        rep, rows = self.run_cmp()
        self.assertEqual(rep.rows, [])
        for sec in ('G1', 'G2', 'G3', 'G4', 'G5', 'G6'):
            self.assertTrue(rep.notes[sec], sec)

    def test_surface_md_lists_served_changes_first_and_keeps_all_rows_in_the_tsv(self):
        calls_old = CALLS_HEAD + ''.join(call('gamma', 'M%03d' % i, 1, served='no') for i in range(60)) + call('alpha', 'GetThing', 1)
        calls_new = CALLS_HEAD + call('alpha', 'GetThing', 2)
        self.both('client-calls.tsv', calls_old, calls_new)
        rc = ps.main([self.old.dir, self.new.dir, '--out', self.out, '--repo', self.repo])
        self.assertEqual(rc, 3)
        md = rd(self.out + '/surface.md')
        first = [ln for ln in md.splitlines() if ln.startswith('- ')]
        g1 = [ln for ln in first if 'alpha.GetThing' in ln or 'gamma.M' in ln]
        self.assertIn('**SERVED** arity `alpha.GetThing`', g1[0])
        self.assertIn('and 21 more', md)
        tsv = rd(self.out + '/surface-diff.tsv').splitlines()
        self.assertEqual(tsv[0], 'section\tchange\tkey\tdetail\tserved\timpact')
        self.assertEqual(len(tsv) - 1, 61)

    def test_unknown_directory_is_a_usage_error(self):
        with self.assertRaises(SystemExit):
            ps.main([self.old.dir, os.path.join(self.tmp, 'nope'), '--out', self.out])


FAKE_DOCKER = r'''#!/usr/bin/env bash
# Maps /work, /o, /tree onto the fake checkout and writes tiny invented tables.
echo "docker $*" >> "$FAKE_LOG"
[[ $1 == image ]] && exit 0
work="" o="" tree=""
args=("$@")
for ((i = 0; i < ${#args[@]}; i++)); do
    if [[ ${args[i]} == -v ]]; then
        m=${args[i+1]}
        case $m in
            *:/work:ro) work=${m%%:*} ;;
            *:/o) o=${m%%:*} ;;
            *:/tree:ro) tree=${m%%:*} ;;
        esac
    fi
done
# the command starts after the image name
for ((i = 0; i < ${#args[@]}; i++)); do [[ ${args[i]} == eve-offline/analysis:latest ]] && break; done
cmd=("${args[@]:i+1}")
script=""
for a in "${cmd[@]}"; do [[ $a == /work/tools/*.py ]] && { script=$a; break; }; done
base=$(basename "${script:-x}")
case $base in
    scan_bound.py) echo '{"services": {}}' > "$o/bound-objects.json" ;;
    scan_client_callable.py) printf 'service\tclass\tguid\tmodule\tdecl_line\tmethod\tmethod_line\targs\tmetadata\treturns\n' > "$o/client-callable.tsv" ;;
    scan_push.py) printf 'event\thandler_file\tservice\tdeclared_in\tarity\tparams\tpayload_shape\trisk\tnotes\nOnInventedEvent\tinvented/ui.py:5\tS [svc.s]\tnotifyevents\t%s\ta\tpass\tn\tn\n' "$(cat "$tree/arity")" > "$o/push-events.tsv" ;;
    scan_session_keys.py) printf 'key\ttype\torigin\tset_by\treaders\tabsent\twrong\tconfidence\tcitation\n' > "$o/session-keys.tsv" ;;
    scan_user_errors.py) printf 'key\tsource\n' > "$o/user-errors.tsv"; echo "args: ${cmd[*]}" >> "$o/ue-args" ;;
    scan_proto_messages.py) : > "$o/proto-messages.txt" ;;
    gw_dump.py) : > "$o/gw-protos.txt" ;;
    scan_calls_served.py) echo '{"services": ["alpha"], "pairs": ["alpha\tGetThing"]}' > "$o/served.json" ;;
    port_surface.py) exec python3 -I "$work/tools/port_surface.py" "$o/old" "$o/new" --out "$o" --repo "$work" ;;
    *) # scan_remote / scan_services run through `bash -c`
        case "${cmd[*]}" in
            *scan_remote.py*) echo '=== remote services referenced by client (0) ===' ;;
            *scan_services.py*) echo '{}' > "$o/client-services.json" ;;
        esac ;;
esac
exit 0
'''

FAKE_CENSUS = r'''#!/usr/bin/env bash
echo "census-calls $*" >> "$FAKE_LOG"
tree=$2; outfile=$4
printf 'service\tmethod\tcall_site\tkind\tconsumer\tguard\tserved\tnotes\nalpha\tGetThing\ta.py:1\tcall\tuse\tnone\tyes\tin f(), %s args\n' "$(cat "$tree/nargs")" > "$outfile"
'''


class Wrapper(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix='test-port-surface-sh-')
        self.repo = os.path.join(self.tmp, 'repo')
        self.log = os.path.join(self.tmp, 'calls.log')
        open(self.log, 'w').close()
        for f in ('port-surface.sh', 'port_surface.py'):
            os.makedirs(self.repo + '/tools', exist_ok=True)
            shutil.copy(os.path.join(HERE, f), self.repo + '/tools/' + f)
        put(self.repo + '/tools/census-calls.sh', FAKE_CENSUS)
        os.chmod(self.repo + '/tools/census-calls.sh', 0o755)
        put(self.repo + '/server/crates/eve-server/src/service.rs', 'fn f() { send("OnInventedEvent"); }\n')
        put(self.repo + '/findings/session-keys.tsv', 'key\n')
        self.bin = os.path.join(self.tmp, 'bin')
        put(self.bin + '/docker', FAKE_DOCKER)
        os.chmod(self.bin + '/docker', 0o755)
        self.env = dict(os.environ, PATH=self.bin + os.pathsep + os.environ['PATH'], FAKE_LOG=self.log)
        self.env.pop('EVE_SURFACE_OLD_CLIENT', None)
        self.env.pop('EVE_SURFACE_NEW_CLIENT', None)
        self.old = self.tree('3000001', arity='2', nargs='2')
        self.new = self.tree('3000002', arity='2', nargs='2')

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def tree(self, build, arity, nargs):
        d = os.path.join(self.tmp, 'trees', build, 'pyc')
        put(d + '/a.pyc', 'bytes ' + build)
        put(d + '/arity', arity)
        put(d + '/nargs', nargs)
        return d

    def sh(self, *args, env=None):
        return subprocess.run(['bash', self.repo + '/tools/port-surface.sh', *args], capture_output=True, text=True,
                              env=env or self.env)

    def log_lines(self):
        return rd(self.log).splitlines()

    def test_usage_errors_exit_two(self):
        self.assertEqual(self.sh().returncode, 2)
        self.assertEqual(self.sh(self.old).returncode, 2)
        r = self.sh(self.old, os.path.join(self.tmp, 'nope'))
        self.assertEqual(r.returncode, 2)
        self.assertIn('no such tree', r.stderr)
        r = self.sh(self.old, self.old)
        self.assertEqual(r.returncode, 2)
        self.assertIn('same tree', r.stderr)
        r = self.sh(self.old, self.new, '--old-client', os.path.join(self.tmp, 'nope'))
        self.assertEqual(r.returncode, 2)
        self.assertIn('no such install', r.stderr)
        r = self.sh(self.old, self.new, '--frobnicate')
        self.assertEqual(r.returncode, 2)
        self.assertEqual(self.log_lines(), [], 'a usage error runs nothing')

    def test_default_out_is_beside_the_new_tree_and_a_clean_run_exits_zero(self):
        r = self.sh(self.old, self.new)
        self.assertEqual(r.returncode, 0, r.stderr + r.stdout)
        out = os.path.dirname(self.new) + '/surface'
        self.assertTrue(os.path.isfile(out + '/surface.md'))
        self.assertTrue(os.path.isfile(out + '/surface-diff.tsv'))
        for side in ('old', 'new'):
            for f in ('client-calls.tsv', 'bound-objects.json', 'remote-services.txt', 'client-services.json',
                      'client-callable.tsv', 'push-events.tsv', 'session-keys.tsv', 'user-errors.tsv',
                      'proto-messages.txt', 'gw-protos.txt', 'meta.txt'):
                self.assertTrue(os.path.isfile('%s/%s/%s' % (out, side, f)), f)
        self.assertIn('label=3000002', rd(out + '/new/meta.txt'))
        self.assertIn('no served impact', r.stdout)

    def test_a_served_change_exits_three(self):
        put(self.new + '/nargs', '3')
        r = self.sh(self.old, self.new, '--out', os.path.join(self.tmp, 'o'))
        self.assertEqual(r.returncode, 3, r.stderr + r.stdout)
        self.assertIn('**SERVED** arity `alpha.GetThing`', rd(self.tmp + '/o/surface.md'))
        self.assertIn('SERVED IMPACT', r.stdout)

    def test_the_cache_serves_an_unchanged_side_and_a_changed_tree_misses(self):
        out = os.path.join(self.tmp, 'o')
        self.assertEqual(self.sh(self.old, self.new, '--out', out).returncode, 0)
        runs = len([ln for ln in self.log_lines() if ln.startswith('census-calls')])
        self.assertEqual(runs, 2)
        r = self.sh(self.old, self.new, '--out', out)
        self.assertEqual(r.returncode, 0)
        self.assertEqual(len([ln for ln in self.log_lines() if ln.startswith('census-calls')]), 2, 'both sides cached')
        self.assertEqual(r.stdout.count(': cached'), 2)
        put(self.new + '/a.pyc', 'different bytes')
        self.sh(self.old, self.new, '--out', out)
        self.assertEqual(len([ln for ln in self.log_lines() if ln.startswith('census-calls')]), 3, 'only the changed side reruns')
        self.sh(self.old, self.new, '--out', out, '--no-cache')
        self.assertEqual(len([ln for ln in self.log_lines() if ln.startswith('census-calls')]), 5)

    def test_without_an_install_user_errors_read_bytecode_only(self):
        out = os.path.join(self.tmp, 'o')
        self.assertEqual(self.sh(self.old, self.new, '--out', out).returncode, 0)
        self.assertIn('--no-resources', rd(out + '/old/ue-args'))
        self.assertIn('resources=no', rd(out + '/old/meta.txt'))
        client = os.path.join(self.tmp, 'EVE', 'tq-3000001')
        put(client + '/start.ini', '[main]\nbuild = 3000001\n')
        self.assertEqual(self.sh(self.old, self.new, '--out', out, '--old-client', client).returncode, 0)
        args = rd(out + '/old/ue-args')
        self.assertIn('--client /client', args)
        self.assertNotIn('--no-resources', args)
        self.assertIn('client_build=3000001', rd(out + '/old/meta.txt'))
        self.assertIn('resources=no', rd(out + '/new/meta.txt'))

    def test_the_tool_is_executable_and_parses(self):
        p = os.path.join(HERE, 'port-surface.sh')
        self.assertTrue(os.stat(p).st_mode & stat.S_IXUSR)
        self.assertEqual(subprocess.run(['bash', '-n', p]).returncode, 0)


if __name__ == '__main__':
    unittest.main()
