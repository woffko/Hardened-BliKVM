/* SPDX-License-Identifier: GPL-3.0-only */
/*
 * An empty library. The traced library names three companions it must be
 * loaded with; what it wants of them is in the trace program itself
 * (vendor_ops.c, vendor_ion.c), so each companion's name is given to a
 * copy of this.
 */
int ve_trace_companion_stub;
