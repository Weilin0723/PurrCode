import assert from "node:assert/strict";
import { page } from "./pagination.mjs";

assert.deepEqual(page([0, 1, 2, 3, 4], 2, 2), [2, 3]);
