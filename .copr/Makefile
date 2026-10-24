# COPR builds this fork with its "custom" source method: it clones the
# repository, runs
#
#     make -f .copr/Makefile srpm outdir=<directory>
#
# and rebuilds the one SRPM that turns up there in every enabled chroot. The
# project page and the GitHub Action that submits builds after a release are
# documented in README.md ("Installing from COPR").
#
# Unlike packaging/build-rpm.sh, which builds from the working tree and passes
# --define 'ipp_version <version>', an SRPM has to be self-contained: it is
# rebuilt on another machine, with no working tree to read the version from and
# no --define to hand it. So the version is read here and baked into a copy of
# the spec, and the source tarball is versioned accordingly.
#
# The tarball holds the same file list as the distro scripts. locales/ has to be
# in there: rust-i18n compiles the catalogs into the binaries, so the build
# fails without them.
#
# Every rewrite is asserted to hit exactly one line: if the spec changes shape
# this target fails instead of quietly shipping a wrong tarball name.

specfile := packaging/rpm/ipp-duplexd.spec
version  := $(shell sed -n 's/^version = "\(.*\)"$$/\1/p' Cargo.toml)
distname := ipp-duplexd-$(version)
sources  := Cargo.toml Cargo.lock src locales README.md LICENSE \
            packaging/ipp-duplexd.service packaging/ipp-duplexd.conf.example
outdir   := $(abspath $(outdir))

.PHONY: srpm check

srpm: check
	mkdir -p "$(outdir)"
	work="$$(mktemp -d)"; trap 'rm -rf "$$work"' EXIT; \
	tar czf "$$work/$(distname).tar.gz" --transform 's,^,$(distname)/,' $(sources); \
	sed -e 's|%{ipp_version}|$(version)|g' \
	    -e '/^%{!?ipp_version:/d' \
	    -e 's|^Source0:.*|Source0:        $(distname).tar.gz|' \
	    -e 's|^%setup -q -n ipp-duplexd$$|%setup -q -n $(distname)|' \
	    "$(specfile)" > "$$work/$(distname).spec"; \
	rpmbuild -bs --define "_topdir $$work/rpmbuild" \
	             --define "_sourcedir $$work" \
	             --define "_srcrpmdir $(outdir)" \
	             "$$work/$(distname).spec"; \
	set -- "$(outdir)"/*.src.rpm; \
	if [ "$$#" -ne 1 ]; then echo "expected exactly one SRPM in $(outdir), found $$#"; exit 1; fi; \
	srpm="$$1"; contents="$$(rpm -qpl "$$srpm")"; \
	case "$$contents" in *"$(distname).tar.gz"*) ;; *) echo "$$srpm is missing $(distname).tar.gz"; exit 1;; esac; \
	case "$$contents" in *"$(distname).spec"*) ;; *) echo "$$srpm is missing $(distname).spec"; exit 1;; esac; \
	echo "$$srpm"

# Guards: a missing version, a missing outdir, or a spec that no longer has
# exactly one rewrite anchor line each would otherwise produce a subtly wrong
# package instead of an error.
check:
	@test -n "$(version)" || { echo "no 'version = \"...\"' line in Cargo.toml"; exit 1; }
	@test -n "$(outdir)"  || { echo "pass outdir=<directory>, COPR does"; exit 1; }
	@test "$$(grep -c '%{ipp_version}' $(specfile))" = 1 || { echo "spec: %{ipp_version} must appear once"; exit 1; }
	@test "$$(grep -c '^%{!?ipp_version:' $(specfile))" = 1 || { echo "spec: the ipp_version guard must appear once"; exit 1; }
	@test "$$(grep -c '^Source0:' $(specfile))" = 1 || { echo "spec: Source0 must appear once"; exit 1; }
	@test "$$(grep -c '^%setup -q -n ipp-duplexd$$' $(specfile))" = 1 || { echo "spec: '%setup -q -n ipp-duplexd' must appear once"; exit 1; }
	@echo "ipp-duplexd $(version) -> $(outdir)"
