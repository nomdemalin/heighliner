# Heighliner — front-door Makefile.
#
# House rule: `make` with no target prints grouped help from ## comments.
# All real work defers to cargo / cargo-bundle; this file is the orchestration
# layer so anyone (including future me) can `make dev` without memorizing flags.

APP_NAME       := Heighliner
BIN_NAME       := heighliner
TARGET_DIR     := target
RELEASE_DIR    := $(TARGET_DIR)/release
DIST_DIR       := dist
INSTALL_DIR    := /Applications
CARGO          := cargo
RUST_LOG       ?= info,heighliner=debug
ICON_SVG       := assets/icon.svg
ICONSET_DIR    := assets/$(APP_NAME).iconset
ICNS           := assets/$(APP_NAME).icns

.DEFAULT_GOAL := help

# ---------------------------------------------------------------- help ---
.PHONY: help
help: ## Show this help
	@awk 'BEGIN {FS = ":.*?## "}; \
	     /^## [A-Z]/ {printf "\n\033[1m%s\033[0m\n", substr($$0, 4); next} \
	     /^[a-zA-Z0-9_.-]+:.*?## / {printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)

## Setup
.PHONY: setup
setup: ## Install the toolchain and build deps
	@rustup show > /dev/null
	@$(CARGO) --version
	@echo "If you don't have cargo-bundle, run: cargo install cargo-bundle"

## Development
.PHONY: dev
dev: ## Run in dev mode (debug build, verbose logs)
	RUST_LOG=$(RUST_LOG) $(CARGO) run

.PHONY: fmt
fmt: ## Format the project
	$(CARGO) fmt

.PHONY: lint
lint: ## cargo clippy with warnings as errors
	$(CARGO) clippy --all-targets -- -D warnings

.PHONY: check
check: ## cargo check (fast type-check)
	$(CARGO) check --all-targets

## Build
.PHONY: build
build: ## Release build of the binary (no side effects outside target/)
	$(CARGO) build --release

## Assets
ICON_PNG := assets/icon.png

.PHONY: icon
icon: $(ICNS) $(ICON_PNG) ## Render assets/icon.svg → .icns + 512px PNG (needs: brew install librsvg)

$(ICON_PNG): $(ICON_SVG)
	@command -v rsvg-convert >/dev/null || { echo "rsvg-convert not found. Install with: brew install librsvg"; exit 1; }
	rsvg-convert -w 512 -h 512 $(ICON_SVG) -o $(ICON_PNG)
	@echo "Built $(ICON_PNG)"

$(ICNS): $(ICON_SVG)
	@command -v rsvg-convert >/dev/null || { echo "rsvg-convert not found. Install with: brew install librsvg"; exit 1; }
	@command -v iconutil >/dev/null || { echo "iconutil missing (expected on macOS). Try Xcode command-line tools."; exit 1; }
	rm -rf $(ICONSET_DIR)
	mkdir -p $(ICONSET_DIR)
	@for s in 16 32 128 256 512; do \
	  s2=$$(( s * 2 )); \
	  rsvg-convert -w $$s -h $$s $(ICON_SVG) -o $(ICONSET_DIR)/icon_$${s}x$${s}.png; \
	  rsvg-convert -w $$s2 -h $$s2 $(ICON_SVG) -o $(ICONSET_DIR)/icon_$${s}x$${s}@2x.png; \
	done
	iconutil -c icns -o $(ICNS) $(ICONSET_DIR)
	rm -rf $(ICONSET_DIR)
	@echo "Built $(ICNS)"

BUNDLE_ID      := io.github.nomdemalin.heighliner
APP_VERSION    := $(shell awk -F'"' '/^version/ {print $$2; exit}' Cargo.toml)
APP_DIR        := $(DIST_DIR)/$(APP_NAME).app

.PHONY: bundle
bundle: build $(ICNS) ## Build a macOS .app bundle under dist/
	rm -rf $(APP_DIR)
	mkdir -p $(APP_DIR)/Contents/MacOS
	mkdir -p $(APP_DIR)/Contents/Resources
	cp $(RELEASE_DIR)/$(BIN_NAME) $(APP_DIR)/Contents/MacOS/$(BIN_NAME)
	cp $(ICNS) $(APP_DIR)/Contents/Resources/$(APP_NAME).icns
	@printf '%s\n' \
	  '<?xml version="1.0" encoding="UTF-8"?>' \
	  '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
	  '<plist version="1.0">' \
	  '<dict>' \
	  '  <key>CFBundleName</key><string>$(APP_NAME)</string>' \
	  '  <key>CFBundleDisplayName</key><string>$(APP_NAME)</string>' \
	  '  <key>CFBundleIdentifier</key><string>$(BUNDLE_ID)</string>' \
	  '  <key>CFBundleVersion</key><string>$(APP_VERSION)</string>' \
	  '  <key>CFBundleShortVersionString</key><string>$(APP_VERSION)</string>' \
	  '  <key>CFBundleExecutable</key><string>$(BIN_NAME)</string>' \
	  '  <key>CFBundleIconFile</key><string>$(APP_NAME).icns</string>' \
	  '  <key>CFBundlePackageType</key><string>APPL</string>' \
	  '  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>' \
	  '  <key>LSMinimumSystemVersion</key><string>11.0</string>' \
	  '  <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>' \
	  '  <key>NSHighResolutionCapable</key><true/>' \
	  '  <key>NSPrincipalClass</key><string>NSApplication</string>' \
	  '  <key>NSAppleEventsUsageDescription</key><string>Heighliner reveals files in Finder.</string>' \
	  '</dict>' \
	  '</plist>' \
	  > $(APP_DIR)/Contents/Info.plist
	@# Nudge LaunchServices so the Dock/Finder re-reads the bundle's icon
	@# rather than serving a cached one from an older build.
	@/usr/bin/touch $(APP_DIR)
	@LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister; \
	 if [ -x "$$LSREGISTER" ]; then "$$LSREGISTER" -f "$(APP_DIR)"; fi
	@echo "Bundle: $(APP_DIR)"
	@echo "Note: quit any running Heighliner before relaunching so the new icon takes effect."

.PHONY: package
package: bundle ## Zip the bundle for distribution
	@cd $(DIST_DIR) && zip -qr $(APP_NAME).zip $(APP_NAME).app
	@echo "Package: $(DIST_DIR)/$(APP_NAME).zip"

## Install
.PHONY: install
install: bundle ## Install the .app to /Applications (refuses to clobber)
	@if [ -e "$(INSTALL_DIR)/$(APP_NAME).app" ]; then \
	  echo "Refusing to clobber $(INSTALL_DIR)/$(APP_NAME).app — remove it first or run \`make reinstall\`"; \
	  exit 1; \
	fi
	cp -R $(DIST_DIR)/$(APP_NAME).app $(INSTALL_DIR)/
	@echo "Installed to $(INSTALL_DIR)/$(APP_NAME).app"

.PHONY: reinstall
reinstall: bundle ## Remove and reinstall the .app (destructive)
	rm -rf $(INSTALL_DIR)/$(APP_NAME).app
	cp -R $(DIST_DIR)/$(APP_NAME).app $(INSTALL_DIR)/

.PHONY: uninstall
uninstall: ## Remove the installed .app
	rm -rf $(INSTALL_DIR)/$(APP_NAME).app

## Run
.PHONY: run
run: build ## Run the release build directly
	RUST_LOG=$(RUST_LOG) $(RELEASE_DIR)/$(BIN_NAME)

.PHONY: run-bundle
run-bundle: bundle ## Launch the built .app
	open $(DIST_DIR)/$(APP_NAME).app

## Test
.PHONY: test
test: ## Run tests
	$(CARGO) test

## Inspect
.PHONY: version
version: ## Print the version from Cargo.toml
	@awk -F'"' '/^version/ {print $$2; exit}' Cargo.toml

.PHONY: paths
paths: ## Print the resolved config / data paths Heighliner uses
	@echo "Config: ~/Library/Application Support/heighliner (or XDG)"
	@echo "Data:   ~/Library/Application Support/Heighliner"
	@echo "Index:  ~/Library/Application Support/Heighliner/index.sqlite"

## Clean
.PHONY: clean
clean: ## Remove build artifacts
	$(CARGO) clean
	rm -rf $(DIST_DIR)

.PHONY: distclean
distclean: clean ## Also remove the on-disk index and config (DESTRUCTIVE)
	@echo "This will remove your Heighliner config and index. Ctrl-C to abort; waiting 3s…"
	@sleep 3
	rm -rf "$(HOME)/Library/Application Support/Heighliner"
