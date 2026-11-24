# repo-infra: man v5
#
# Purpose: Build the man page from docs/manual.md (D23).
# Choose: The project ships a man page.
# Supplies: docs/manual.md with section: and date: in its front matter;
#   MAN_NAME in the Makefile.
# Pieces: man-lua
#
# The man page, built from docs/manual.md (D23).
#
# docs/manual.md is the one source. pandoc converts it to
# man/$(MAN_NAME).<section> on demand, and man/ belongs in .gitignore: a generated page under version
# control can disagree with its source. build/man-deflist.lua turns the
# manual's "- `--option`: text" bullet lists into definition lists, so the page
# gets .TP entries while GitHub still renders the source as lists.
#
# repo-infra owns this file. Include it from the Makefile after naming the page:
#
#     MAN_NAME = mytool
#     include build/man.mk
#
# The manual is read as `markdown-smart`. pandoc's default `smart` extension
# turns `--` into an en dash, so an option such as **--api** in running text
# would reach the page as `\[en]api`; code spans are never affected.
#
# The page's section comes from `section:` in the manual's front matter, so a
# daemon whose manual says `section: 8` gets man/$(MAN_NAME).8. The manual names
# its section once and there is no Makefile variable to disagree with it. A
# manual without a usable `section:` line stops `make man` and nothing else.
#
# The page's date comes from `date:` in the manual's front matter, never from
# the build, so two builds of one source produce the same page.
#
# The include may go anywhere in the Makefile; it saves and restores
# .DEFAULT_GOAL so that adding this fragment never makes `man` the target a
# bare `make` builds.
#
# An autotools project includes it from the top-level Makefile.am, where
# automake reads the file itself, and ships it in the tarball:
#
#     MAN_NAME = mytool
#     include $(top_srcdir)/build/man.mk
#     EXTRA_DIST = docs/manual.md build/man.mk build/man-deflist.lua
#
# For that the file has no GNU make conditional (ifeq, else, endif): automake
# stops at one. `:=`, $(shell) and $(if) remain, and automake warns about them
# under -Wportability, so a project with -Werror adds -Wno-portability. The
# paths are relative to the top directory: the build runs in the source tree,
# and a Makefile.am in a subdirectory cannot include the file.


_repo_infra_man_check := $(if $(strip $(MAN_NAME)),,$(error MAN_NAME is not set: set it before `include build/man.mk`, for example MAN_NAME = mytool))

_repo_infra_man_goal := $(.DEFAULT_GOAL)

# Reads `section:` between the opening `---` and the next `---` or `...`, drops
# quotes, and keeps it only if it looks like a man section: 1, 8, 3p.
_repo_infra_man_section := $(shell awk \
  'NR == 1 { if ($$0 != "---") exit; next } \
   /^(---|\.\.\.)[[:space:]]*$$/ { exit } \
   sub(/^section:[[:space:]]*/, "") { gsub(/["\047[:space:]]/, ""); print; exit }' \
  docs/manual.md 2>/dev/null | grep -E '^[1-9][a-z]*$$')

_repo_infra_man_page := man/$(MAN_NAME).$(_repo_infra_man_section)

# Without a section `man` has no prerequisite and its recipe says why; with
# one the recipe is `:` and the page rule below does the work.
.PHONY: man
man: $(if $(_repo_infra_man_section),$(_repo_infra_man_page))
	@$(if $(_repo_infra_man_section),:,echo "build/man.mk: docs/manual.md has no section: line in its front matter that names a man section such as 1 or 8" >&2; exit 1)

$(_repo_infra_man_page): docs/manual.md build/man-deflist.lua
	@mkdir -p man
	pandoc --standalone --from markdown-smart --to man --lua-filter build/man-deflist.lua \
	  docs/manual.md -o $@

.DEFAULT_GOAL := $(_repo_infra_man_goal)
