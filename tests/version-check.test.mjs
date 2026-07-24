import assert from "node:assert/strict";
import test from "node:test";

import { latestChangelogRelease } from "../scripts/check-version.mjs";

test("changelog releases require a complete SemVer heading token", () => {
  for (const heading of [
    "## [0.2.0] - 2026-07-24",
    "## 0.2.0 - 2026-07-24",
  ]) {
    assert.deepEqual(latestChangelogRelease(heading), {
      version: "0.2.0",
      date: "2026-07-24",
    });
  }

  for (const malformed of [
    "## [0.2.0 - 2026-07-24",
    "## 0.2.0] - 2026-07-24",
    "## [0.2.0]junk - 2026-07-24",
    "## 0.2.0junk - 2026-07-24",
  ]) {
    assert.deepEqual(latestChangelogRelease(malformed), {});
  }
});

test("changelog release dates must be real calendar dates", () => {
  assert.deepEqual(
    latestChangelogRelease("## [0.2.0] - 2024-02-29"),
    {
      version: "0.2.0",
      date: "2024-02-29",
    },
  );

  for (const impossibleDate of ["2026-99-99", "2025-02-29", "2026-04-31"]) {
    assert.deepEqual(
      latestChangelogRelease(`## [0.2.0] - ${impossibleDate}`),
      {
        version: "0.2.0",
        date: undefined,
      },
    );
  }
});
