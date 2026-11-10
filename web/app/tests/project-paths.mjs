import assert from "node:assert/strict"
import fs from "node:fs"
import { createRequire } from "node:module"
import path from "node:path"
import vm from "node:vm"
import { fileURLToPath } from "node:url"

const app = path.dirname(path.dirname(fileURLToPath(import.meta.url)))
const require = createRequire(path.join(app, "package.json"))
const ts = require("typescript")
function load(file, globals = {}) {
  const module = { exports: {} }
  const source = fs.readFileSync(path.join(app, "src", file), "utf8")
  const { outputText } = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX } })
  vm.runInNewContext(outputText, {
    module, exports: module.exports, URL, URLSearchParams,
    require: (name) => name === "./Connection" ? {} : require(name), ...globals,
  }, { filename: file })
  return module.exports
}

const { projectPath, rememberProject } = load("app/project.tsx")
const project = { id: "project-canonical-id", code: "ABC12DEF3" }
assert.equal(projectPath(project), "/p/ABC12DEF3")
assert.equal(projectPath(project, "assets/a%20picture"), "/p/ABC12DEF3/assets/a%20picture")
assert.equal(projectPath(project, "/runs?all"), "/p/ABC12DEF3/runs?all")
assert.equal(projectPath({ id: "old id" }), "/projects/old%20id")
const cached = new Map()
rememberProject({ setQueryData: (key, value) => cached.set(key.join("/"), value) }, project)
assert.equal(cached.get("project-reference/ABC12DEF3").id, project.id)
assert.equal(cached.get("project-reference/project-canonical-id").code, project.code)

function consume(url) {
  const location = new URL(url)
  const history = { replaceState(_state, _title, next) {
    location.href = new URL(next, location).href
  } }
  const { takeCode } = load("app/session.ts", { location, history })
  return { code: takeCode(), url: location.href }
}
assert.deepEqual(consume("http://localhost:7317/p/ABC12DEF3#code=secret"), { code: "secret", url: "http://localhost:7317/p/ABC12DEF3" })
assert.deepEqual(consume("http://localhost:7317/p/ABC12DEF3?view=wide#code=secret"), { code: "secret", url: "http://localhost:7317/p/ABC12DEF3?view=wide" })
assert.deepEqual(consume("http://localhost:7317/#/projects/old/assets/picture"), { code: null, url: "http://localhost:7317/projects/old/assets/picture" })
assert.deepEqual(consume("http://localhost:7317/#code=secret&to=/projects/old/runs"), { code: "secret", url: "http://localhost:7317/projects/old/runs" })
assert.deepEqual(consume("http://localhost:7317/p/ABC12DEF3#code=secret&to=//elsewhere.test/path"), { code: "secret", url: "http://localhost:7317/p/ABC12DEF3" })
console.log("project path and auth-fragment checks passed")
