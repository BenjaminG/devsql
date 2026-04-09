import fs from 'node:fs';
import path, { dirname, basename as base } from 'node:path';
import * as chalk from 'chalk';
import type { Table } from './types';
import { formatDate } from './utils/date';
export { formatDate as exportedDate } from './utils/date';
export * as unsafe from './dangerous';

export async function run(table: Table) {
  console.log('demo', table, path.resolve('.'));
}
