import copy
import unittest

from tools.reclaim_application_slot import reviewed_inventory


def inventory(oldest=2):
    slots = [f'app-{n:020d}-' + 'a' * 64 for n in range(oldest, oldest + 8)]
    return {'slots': slots, 'proposedDeletion': slots[:1], 'retainUnreferencedForOneDeletion': 5,
            'status': {'floorSequence': oldest + 7, 'generation': 43 if oldest == 2 else 40,
                       'pending': None, 'activeSlotId': slots[-1], 'confirmedSlotId': slots[-1],
                       'previousSlotId': slots[-2]}}


class ReclaimProfileTests(unittest.TestCase):
    def test_explicit_cycles(self):
        self.assertEqual(reviewed_inventory(inventory(), 'seq2-for-seq10')['oldest'], 2)
        self.assertEqual(reviewed_inventory(inventory(1), 'seq1-for-seq9')['oldest'], 1)

    def test_old_default_does_not_authorize_next_deletion(self):
        with self.assertRaises(ValueError):
            reviewed_inventory(inventory(), 'seq1-for-seq9')

    def test_rejects_drift_and_broader_deletion(self):
        original = inventory()
        for mutation in (
            lambda i: i.update(proposedDeletion=i['slots'][:2]),
            lambda i: i.update(proposedDeletion=i['slots'][1:2]),
            lambda i: i.update(retainUnreferencedForOneDeletion=4),
            lambda i: i['status'].update(generation=44),
            lambda i: i['status'].update(pending={}),
            lambda i: i['status'].update(previousSlotId=i['slots'][0]),
            lambda i: i['slots'].__setitem__(1, i['slots'][0]),
        ):
            changed = copy.deepcopy(original)
            mutation(changed)
            with self.assertRaises(ValueError):
                reviewed_inventory(changed, 'seq2-for-seq10')


if __name__ == '__main__':
    unittest.main()
