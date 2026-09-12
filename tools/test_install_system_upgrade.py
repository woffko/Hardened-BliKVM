"""Host-only tests for embedded upgrade guards; never invoke APT or SSH."""

import ast
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest
from typing import Any

from tools.install_system_upgrade import WORKER, START, OBSERVE


class UpgradeGuardTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        root = Path(self.tmp.name)
        self.scope: dict[str, Any] = {'root': root, 'policy': root / 'policy-rc.d', 'hashlib': hashlib,
                      'json': json, 'os': os, 're': re}
        tree = ast.parse(WORKER)
        nodes: list[ast.stmt] = [node for node in tree.body if isinstance(node, ast.FunctionDef)]
        nodes += [node for node in tree.body if isinstance(node, ast.Assign) and
                  any(isinstance(target, ast.Name) and target.id == 'policy_text' for target in node.targets)]
        exec(compile(ast.Module(body=nodes, type_ignores=[]), 'guard-functions', 'exec'), self.scope)

    def test_programs_compile(self):
        for source in (WORKER, START, OBSERVE):
            compile(source, 'remote', 'exec')

    def test_simulation_accepts_upgrades_only(self):
        self.assertEqual(self.scope['parse_simulation']('Inst foo [1.0] (1.1 Ubuntu [arm64])'), {'foo': '1.1'})
        for text in ('Remv foo [1]', 'Purg foo', 'Inst foo (1.1 Ubuntu [arm64])',
                     'Inst foo [1] (2 x)\nInst foo [1] (2 x)'):
            with self.subTest(text=text), self.assertRaises(ValueError):
                self.scope['parse_simulation'](text)

    def test_policy_only_blocks_reviewed_services(self):
        path = self.scope['policy']
        path.write_text(self.scope['policy_text'])
        for service, expected in (('nginx', 101), ('nginx.service', 101), ('NetworkManager.service', 101),
                                  ('network-manager', 101), ('ssh', 0), ('udev', 0), ('dbus', 0),
                                  ('hardened-blikvm-nginx.service', 0)):
            with self.subTest(service=service):
                result = subprocess.run(['/bin/sh', str(path), service, 'restart'], check=False)
                self.assertEqual(result.returncode, expected)

    def test_cleanup_removes_only_owned_unchanged_policy(self):
        policy = self.scope['policy']
        policy.write_text(self.scope['policy_text'])
        self.scope['cleanup']()
        self.assertTrue(policy.exists())
        self.scope['save']('policy-owned.json', {'owned': True})
        self.scope['cleanup']()
        self.assertFalse(policy.exists())
        self.scope['cleanup']()

    def test_cleanup_refuses_changed_or_symlink_policy(self):
        policy = self.scope['policy']
        self.scope['save']('policy-owned.json', {'owned': True})
        policy.write_text('user replacement')
        with self.assertRaises(ValueError):
            self.scope['cleanup']()
        self.assertEqual(policy.read_text(), 'user replacement')
        policy.unlink()
        target = self.scope['root'] / 'other'
        target.write_text(self.scope['policy_text'])
        policy.symlink_to(target)
        with self.assertRaises(ValueError):
            self.scope['cleanup']()
        self.assertTrue(target.exists())


if __name__ == '__main__':
    unittest.main()
