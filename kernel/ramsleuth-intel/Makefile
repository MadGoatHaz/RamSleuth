# Makefile - out-of-tree build for the ramsleuth_intel kernel module.
#
# Usage:
#   make               build against the running kernel (uname -r)
#   make KVER=<ver>    build against the <ver> kernel's headers
#                      (this is what dkms.conf's MAKE passes)
#   make clean         remove build artifacts
#
# Single-file module (ramsleuth_intel.c); no special CFLAGS needed.
#
# Compiler: clang-built kernels (e.g. CachyOS, CONFIG_CC_IS_CLANG=y)
# must be built with LLVM=1 (their Makefiles carry clang-only flags);
# the detection below sets it automatically from the target kernel's
# .config, so a plain `make` / the dkms.conf MAKE line work on both
# gcc and clang kernels. An explicit LLVM=0/1 on the command line
# wins (make command-line variables override ?= assignments).

KVER ?= $(shell uname -r)
KDIR ?= /lib/modules/$(KVER)/build

ifneq ("$(wildcard $(KDIR)/.config)","")
ifeq ("$(shell grep -q '^CONFIG_CC_IS_CLANG=y' $(KDIR)/.config && echo yes)","yes")
LLVM ?= 1
endif
endif

obj-m := ramsleuth_intel.o

.PHONY: all clean

all:
	$(MAKE) -C $(KDIR) M=$(CURDIR) LLVM=$(LLVM) modules

clean:
	$(MAKE) -C $(KDIR) M=$(CURDIR) clean
