import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "path";
import { resolve } from "path";
export default defineConfig({
    plugins: [react()],
    resolve: {
        alias: {
            "@": path.resolve(__dirname, "./src"),
        },
    },
    build: {
        rollupOptions: {
            input: {
                main: resolve(__dirname, "index.html"),
                // 实时字幕悬浮窗（Tauri 独立 WebviewWindow 加载，纯 vanilla 无 React）
                caption: resolve(__dirname, "caption.html"),
            },
        },
    },
    server: {
        strictPort: true,
        port: 1420,
    },
    clearScreen: false,
    envPrefix: ["VITE_", "TAURI_"],
});
