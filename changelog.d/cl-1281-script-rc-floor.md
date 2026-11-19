A script, or a package whose pyproject.toml has no `[tool.uv]`, with a `cozy-runtime` floor only the open RC meets locks the RC's
Runtime build, as a package with a `[tool.uv]` does. Its lock had lost the RC wheel's source and failed on PyPI's releases.
