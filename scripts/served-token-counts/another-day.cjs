/**
 * @license
 * Copyright 2026 Qwen
 * SPDX-License-Identifier: Apache-2.0
 */

// Preloaded (NODE_OPTIONS=--require) into every Node process of the second
// and third runs ./record.sh makes, so the tests run as they would on another
// day: every clock a test reads moves by ANOTHER_DAY_MS together. A text that
// names when it was counted -- a date, an hour, a number of days since a fixed
// instant -- then differs from the first run's, and the recording is refused,
// because the image's build runs the tests on a day of its own. An age a test
// measures against a file it wrote does not differ, because the file's times
// move with the clock: Node reads them as stat's times and sets them with
// utimes, and both are moved here. Moving the clock alone would make a file a
// test has just written as old as the move, which no run of the tests sees.
//
// Moved: Date (now, a Date made with no argument, Date called as a function),
// Intl's formatting of a date left out, and every fs stat and utimes entry
// point, callback, synchronous, promise and file handle alike. Not moved: the
// clocks of programs other than Node that a test runs, which read the kernel's.
'use strict';

const fs = require('node:fs');
const { syncBuiltinESMExports } = require('node:module');

const shift = Number(process.env.ANOTHER_DAY_MS);
if (!Number.isSafeInteger(shift)) {
  throw new Error(
    `ANOTHER_DAY_MS is ${JSON.stringify(process.env.ANOTHER_DAY_MS)}, not a whole number of milliseconds`,
  );
}

const RealDate = Date;
const realNow = RealDate.now.bind(RealDate);
const now = () => realNow() + shift;
RealDate.now = now;
globalThis.Date = new Proxy(RealDate, {
  construct(target, args, newTarget) {
    return Reflect.construct(target, args.length === 0 ? [now()] : args, newTarget);
  },
  apply() {
    return new RealDate(now()).toString();
  },
});

const format = Object.getOwnPropertyDescriptor(Intl.DateTimeFormat.prototype, 'format');
Object.defineProperty(Intl.DateTimeFormat.prototype, 'format', {
  ...format,
  get() {
    const bound = format.get.call(this);
    return (date) => bound(date === undefined ? now() : date);
  },
});
const formatToParts = Intl.DateTimeFormat.prototype.formatToParts;
Intl.DateTimeFormat.prototype.formatToParts = function (date) {
  return formatToParts.call(this, date === undefined ? now() : date);
};

// A file's times as Node reports them: the kernel's, moved.
const TIMES = ['atime', 'mtime', 'ctime', 'birthtime'];
function moved(stats) {
  if (stats === undefined || stats === null) return stats;
  if (typeof stats.mtimeMs === 'bigint') {
    for (const time of TIMES) {
      stats[`${time}Ms`] += BigInt(shift);
      stats[`${time}Ns`] += BigInt(shift) * 1_000_000n;
    }
  } else {
    for (const time of TIMES) stats[`${time}Ms`] += shift;
  }
  return stats;
}
// A time a test sets, as the kernel must hold it. Node takes a Date, a number
// of seconds or a numeric string, and a negative number as now, which is the
// kernel's own and so already unmoved; anything else Node refuses as it is.
function unmoved(time) {
  let ms;
  if (time instanceof RealDate) ms = time.getTime();
  else if (typeof time === 'number' && Number.isFinite(time) && time >= 0) ms = time * 1000;
  else if (typeof time === 'string' && time.trim() !== '' && Number(time) >= 0) ms = Number(time) * 1000;
  else return time;
  return new RealDate(ms - shift);
}

for (const name of ['stat', 'lstat', 'fstat']) {
  const callback = fs[name];
  fs[name] = function (...args) {
    const done = args[args.length - 1];
    if (typeof done === 'function') {
      args[args.length - 1] = (error, stats) => done(error, error ? stats : moved(stats));
    }
    return callback.apply(this, args);
  };
  const sync = fs[`${name}Sync`];
  fs[`${name}Sync`] = function (...args) {
    return moved(sync.apply(this, args));
  };
}
for (const name of ['stat', 'lstat']) {
  const promised = fs.promises[name];
  fs.promises[name] = async function (...args) {
    return moved(await promised.apply(this, args));
  };
}
for (const name of ['utimes', 'futimes', 'lutimes']) {
  const callback = fs[name];
  fs[name] = function (target, atime, mtime, ...rest) {
    return callback.call(this, target, unmoved(atime), unmoved(mtime), ...rest);
  };
  const sync = fs[`${name}Sync`];
  fs[`${name}Sync`] = function (target, atime, mtime, ...rest) {
    return sync.call(this, target, unmoved(atime), unmoved(mtime), ...rest);
  };
}
for (const name of ['utimes', 'lutimes']) {
  const promised = fs.promises[name];
  fs.promises[name] = function (target, atime, mtime, ...rest) {
    return promised.call(this, target, unmoved(atime), unmoved(mtime), ...rest);
  };
}
// A file handle's stat and utimes, on the prototype of the first handle opened.
const open = fs.promises.open;
let handlesMoved = false;
fs.promises.open = async function (...args) {
  const handle = await open.apply(this, args);
  if (!handlesMoved) {
    handlesMoved = true;
    const prototype = Object.getPrototypeOf(handle);
    const stat = prototype.stat;
    prototype.stat = async function (...rest) {
      return moved(await stat.apply(this, rest));
    };
    const utimes = prototype.utimes;
    prototype.utimes = function (atime, mtime) {
      return utimes.call(this, unmoved(atime), unmoved(mtime));
    };
  }
  return handle;
};

syncBuiltinESMExports();
