# One command to build everything on Linux. Run `make help` to see the targets.
#   make            build the node, the tray icon and the speaker helper (release, CUDA when you have an NVIDIA card)
#   make cpu        the same without CUDA (a Raspberry Pi, a machine without an NVIDIA card)
#   make test       run every test that needs no phone
#   make install    build, then install the node, the tray and the helper for this user and start them
#   make app        build the phone app (needs the Android tools and the models, see docs/DEVELOPMENT.md)
SHELL := /bin/bash
.DEFAULT_GOAL := all

CARGO ?= cargo
GRADLE ?= ./gradlew
NODE_BUILD := scripts/node/build.sh

.PHONY: help all cpu node tray speakers test test-core test-node test-app install models app app-debug clean

help:
	@echo "make            node, tray icon and speaker helper (CUDA)"
	@echo "make cpu        the same without CUDA"
	@echo "make node       only the node (and the speaker helper)"
	@echo "make tray       only the tray icon"
	@echo "make speakers   only the optional speaker helper"
	@echo "make test       all tests that need no phone"
	@echo "make install    build and install for this user, then start the node and the tray"
	@echo "make models     download the node's models (add ONLY=speakers or ONLY=vision for the optional ones)"
	@echo "make app        the phone app (release, with the models inside), needs the Android tools"
	@echo "make app-debug  the small phone app for development"
	@echo "make clean      remove the build folders"

all: node tray

cpu:
	$(NODE_BUILD) --cpu-only
	$(CARGO) build --release -p marginalia-node-tray

# build.sh builds the node and then the optional speaker helper, and does not fail when only the helper cannot be built.
node:
	$(NODE_BUILD)

tray:
	$(CARGO) build --release -p marginalia-node-tray

speakers:
	$(CARGO) build --release -p marginalia-node-speakers

test: test-node test-core test-app

test-node:
	$(CARGO) test -p marginalia-proto -p marginalia-node -p marginalia-node-tray -p marginalia-node-speakers

test-core:
	cd core && $(CARGO) test --lib

test-app:
	$(GRADLE) --offline :app:testDebugUnitTest

install: all
	scripts/node/install-linux.sh

# `make models ONLY=speakers` fetches the speaker models, ONLY=vision the one that lets the node read photos. Without it the three needed models.
models: node
	target/release/marginalia-node models download $(if $(ONLY),--only $(ONLY),)

app:
	scripts/app/build-native.sh arm64
	$(GRADLE) assembleRelease
	@ls -la app/build/outputs/apk/release/*.apk

app-debug:
	scripts/app/build-native.sh arm64
	$(GRADLE) assembleDebug -Pmarginalia.embedModels=false

clean:
	$(CARGO) clean
	$(GRADLE) clean
