import { defineConfig } from 'vite';
import path from 'node:path';
export default defineConfig({ publicDir: process.env.CANVAS_BENCH_ASSETS || path.resolve('.cache/assets'), build:{target:'es2022'}, server:{host:'127.0.0.1',port:5197,strictPort:true}, preview:{host:'127.0.0.1',port:5197,strictPort:true} });
