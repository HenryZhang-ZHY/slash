# Slash Logo Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create matching light- and dark-mode SVG logo files for Slash.

**Architecture:** Both assets are standalone, transparent SVG documents with identical geometry. The only difference is polygon fill: black for light surfaces and white for dark surfaces.

**Tech Stack:** SVG 1.1-compatible XML

---

### Task 1: Create the paired Slash marks

**Files:**
- Create: `assets/logo.svg`
- Create: `assets/logo-dark.svg`

- [ ] **Step 1: Verify the assets do not already exist**

Run:

```powershell
Test-Path assets/logo.svg
Test-Path assets/logo-dark.svg
```

Expected: both output `False`.

- [ ] **Step 2: Create the light-mode SVG**

Create `assets/logo.svg`:

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" preserveAspectRatio="xMidYMid meet" role="img" aria-labelledby="title">
  <title id="title">Slash</title>
  <polygon fill="#000000" points="148,20 198,20 108,236 58,236"/>
</svg>
```

- [ ] **Step 3: Create the dark-mode SVG**

Create `assets/logo-dark.svg`:

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" preserveAspectRatio="xMidYMid meet" role="img" aria-labelledby="title">
  <title id="title">Slash</title>
  <polygon fill="#ffffff" points="148,20 198,20 108,236 58,236"/>
</svg>
```

- [ ] **Step 4: Validate structure and paired geometry**

Run:

```powershell
[xml]$light = Get-Content assets/logo.svg -Raw
[xml]$dark = Get-Content assets/logo-dark.svg -Raw
if ($light.svg.viewBox -ne '0 0 256 256') { throw 'Invalid light viewBox' }
if ($dark.svg.viewBox -ne '0 0 256 256') { throw 'Invalid dark viewBox' }
if ($light.svg.polygon.points -ne $dark.svg.polygon.points) { throw 'Geometry differs' }
if ($light.svg.polygon.fill -ne '#000000') { throw 'Invalid light fill' }
if ($dark.svg.polygon.fill -ne '#ffffff') { throw 'Invalid dark fill' }
```

Expected: exit code 0 with no output.

- [ ] **Step 5: Review and commit**

Run:

```powershell
git diff --check
git diff -- assets/logo.svg assets/logo-dark.svg
git add assets/logo.svg assets/logo-dark.svg
git commit -m "feat: add Slash logo assets"
```

Expected: the diff contains only the two approved SVG assets.
