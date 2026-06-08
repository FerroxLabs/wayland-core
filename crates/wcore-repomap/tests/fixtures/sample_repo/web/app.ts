// Sample TypeScript file for wcore-repomap fixture tests.

import { readFile } from "node:fs/promises";
import * as path from "node:path";

export function greet(name: string): string {
  return `hi ${name}`;
}

export class Widget {
  constructor(public id: number) {}
}

export interface Options {
  verbose: boolean;
}

export type Callback = (n: number) => void;

export const PI = 3.14;

export default class App {}
