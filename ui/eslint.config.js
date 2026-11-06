// ESLint flat config（质量加固计划 Q-04）
//
// 只开「有信息量」的规则，不继承 recommended 全套——这个仓库的痛点是
// 「`invoke()` 漏 await = 静默失败」，不是风格。真正需要的规则就下面这几条。
//
// 用法：`npm --prefix ui run lint`（需要类型信息的规则要看 ui/tsconfig.json）。
import tseslint from "typescript-eslint";

export default [
  { ignores: ["dist/**"] },
  {
    files: ["src/**/*.ts"],
    languageOptions: {
      parser: tseslint.parser,
      parserOptions: {
        // 让 parser 自己找 tsconfig（比 project 手写路径更抗目录搬迁）
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
    plugins: {
      "@typescript-eslint": tseslint.plugin,
    },
    rules: {
      // 漏 await 的 Promise 会变成静默失败——这个 UI 到处 invoke()，最值钱的一条。
      "@typescript-eslint/no-floating-promises": "error",
      // 把返回 Promise 的函数塞进 void 回调 / 事件处理器，同样会吞掉失败。
      "@typescript-eslint/no-misused-promises": "error",
      // 目前 0 处；开着防回退（类型安全的第一道闸）。
      "@typescript-eslint/no-explicit-any": "error",
      // `==` 不做隐式转换；与 null 比较保留（`x == null` 同时挡 null / undefined）。
      eqeqeq: ["error", "always", { null: "ignore" }],
      "prefer-const": "error",
    },
  },
];
