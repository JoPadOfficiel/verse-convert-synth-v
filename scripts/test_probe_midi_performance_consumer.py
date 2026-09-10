"""Synthetic geometry tests; native consumer execution remains a separate gate."""
import copy
import importlib.util
import json
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location(
    "performance_probe", Path(__file__).with_name("probe-midi-performance-consumer.py"))
PROBE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROBE)


def fade_oracle(duration, *, start=0, padding=0, direction="out", peak=1.0):
    gains = [peak * ((duration - tick) / duration if direction == "out" else tick / duration)
             for tick in range(duration + 1)]
    gains.extend([gains[-1]] * padding)
    return {"schema": 1, "ticksPerQuarter": 480, "tickStep": 1,
            "startTick": start, "endTick": start + duration + padding,
            "noteCount": 1, "gains": gains,
            "nienteFadeIntervals": [{"startTick": start, "endTick": start + duration,
                                     "direction": direction}]}


class ScoreOracleMutations(unittest.TestCase):
    def test_actual_target_floor_mutation_preserves_oracle_and_other_values(self):
        oracle = fade_oracle(480)
        before = copy.deepcopy(oracle)
        target = ('voice_parts:\n    curves:\n'
                  '      - abbr: "dyn"\n        xs: [0, 450, 479, 480]\n'
                  '        ys: [0, -239, -239, -240]\n'
                  '      - abbr: "pitd"\n        xs: [0, 480]\n'
                  '        ys: [-239, -239]\n')
        mutations = list(PROBE.score_target_mutations(target, oracle))
        self.assertEqual(len(mutations), 1)
        name, mutated, reason = mutations[0]
        self.assertEqual(name, "wrong-target-floor")
        self.assertEqual(reason, "Score authorized floor must equal DYN -239")
        self.assertEqual(mutated, target.replace('ys: [0, -239, -239, -240]',
                                                'ys: [0, -238, -238, -240]'))
        self.assertEqual(oracle, before)

    def test_target_floor_mutation_requires_actual_floor_and_keeps_floor_free_controls(self):
        self.assertEqual(list(PROBE.score_target_mutations("unused", fade_oracle(10))), [])
        with self.assertRaisesRegex(ValueError, "no emitted DYN -239"):
            list(PROBE.score_target_mutations("no floor", fade_oracle(480)))

    def mutations(self, oracle):
        before = copy.deepcopy(oracle)
        mutations = {name: (json.loads(value), reason)
                     for name, value, reason in PROBE.score_oracle_mutations(oracle)}
        self.assertEqual(oracle, before, "the authored oracle is immutable")
        return mutations

    def test_floor_free_fade_has_positive_missing_authorization_control(self):
        oracle = fade_oracle(5, start=97, padding=5)
        self.assertEqual(PROBE.oracle_geometry(oracle), [])
        mutations = self.mutations(oracle)
        self.assertNotIn("missing-fade-authorization", mutations)
        controls = list(PROBE.score_oracle_controls(oracle))
        self.assertEqual([name for name, _ in controls], ["floor-free-without-authorization"])
        control = json.loads(controls[0][1])
        self.assertEqual(control["gains"], oracle["gains"])
        self.assertEqual(control["nienteFadeIntervals"], [])
        self.assertEqual(PROBE.oracle_geometry(control), [])
        self.assertEqual(mutations["wrong-final-value"][1], "Positive score gain became mute")

    def test_authored_floor_requires_authorization_only_at_actual_interior_ticks(self):
        oracle = fade_oracle(480, start=97, padding=5)
        ticks = PROBE.oracle_geometry(oracle)
        self.assertTrue(ticks)
        self.assertTrue(all(97 < tick < 577 for tick in ticks))
        mutated, reason = self.mutations(oracle)["missing-fade-authorization"]
        self.assertEqual(reason, "Score gain floor outside authored niente interval")
        self.assertEqual(mutated["gains"], oracle["gains"])
        self.assertEqual(mutated["nienteFadeIntervals"], [])
        with self.assertRaisesRegex(ValueError, "^Score gain floor outside authored niente interval$"):
            PROBE.oracle_geometry(mutated)
        self.assertEqual(list(PROBE.score_oracle_controls(oracle)), [])

    def test_final_end_fades_fail_endpoint_authentication_before_sampling(self):
        for direction in ("in", "out"):
            for duration in (5, 480):
                with self.subTest(direction=direction, duration=duration):
                    oracle = fade_oracle(duration, direction=direction)
                    mutated, reason = self.mutations(oracle)["wrong-final-value"]
                    self.assertEqual(reason, "Score oracle invalid niente endpoint")
                    with self.assertRaisesRegex(ValueError, "^Score oracle invalid niente endpoint$"):
                        PROBE.oracle_geometry(mutated)

    def test_ordinary_endpoint_mutation_reaches_exact_sample_failure(self):
        oracle = fade_oracle(5)
        oracle["gains"] = [1.0] * 6
        oracle["nienteFadeIntervals"] = []
        self.assertEqual(self.mutations(oracle)["wrong-final-value"][1], "Score mute endpoint mismatch")

    def test_ordinary_tiny_positive_gains_never_receive_floor_authorization(self):
        for gain in (1e-4, 1e-10):
            self.assertTrue(PROBE.needs_floor(gain))
            oracle = fade_oracle(5)
            oracle["gains"] = [gain] * 6
            oracle["nienteFadeIntervals"] = []
            with self.assertRaisesRegex(ValueError, "^Score gain floor outside authored niente interval$"):
                list(PROBE.score_oracle_mutations(oracle))
        self.assertFalse(PROBE.needs_floor(0))
        self.assertFalse(PROBE.needs_floor(0.064))
        mutations = self.mutations(fade_oracle(5))
        ordinary, reason = mutations["ordinary-small-gain"]
        self.assertEqual(reason, "Score gain floor outside authored niente interval")
        self.assertEqual(ordinary["nienteFadeIntervals"], [])
        forged, reason = mutations["forged-fade-floor"]
        self.assertEqual(reason, "Score oracle invalid niente endpoint")
        with self.assertRaisesRegex(ValueError, "^Score oracle invalid niente endpoint$"):
            PROBE.oracle_geometry(forged)

    def test_mutations_keep_precise_domain_and_finite_value_failure_reasons(self):
        oracle = fade_oracle(5, start=11)
        mutations = self.mutations(oracle)
        for name in ("empty-oracle", "truncated-oracle", "oversized-oracle"):
            self.assertEqual(mutations[name][1], "Score oracle must cover every tick")
        for name in ("wrong-end", "wrong-start", "wrong-step", "wrong-timebase"):
            self.assertEqual(mutations[name][1], "Score oracle domain mismatch")
        for name in ("nonfinite-gain", "negative-gain"):
            self.assertEqual(mutations[name][1], "Score oracle invalid gain")
        for name, (mutated, _) in mutations.items():
            if name not in ("empty-oracle", "truncated-oracle", "oversized-oracle"):
                self.assertEqual(len(mutated["gains"]), len(oracle["gains"]))


if __name__ == "__main__":
    unittest.main()
