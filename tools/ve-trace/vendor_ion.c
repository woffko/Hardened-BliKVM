/* SPDX-License-Identifier: GPL-3.0-only */
/*
 * The names the library imports from its allocator's helper for buffers
 * shared by file descriptor. The trace gives it no such buffer; a call is
 * recorded and refused. Their parameters are not known here and not used.
 */
#include "log.h"

#define REFUSE(name)                                        \
	int name()                                          \
	{                                                   \
		log_add(REC_OP, #name ": refused", 0, 0, 0); \
		return -1;                                  \
	}

REFUSE(CdcIonOpen)
REFUSE(CdcIonClose)
REFUSE(CdcIonImport)
REFUSE(CdcIonFree)
REFUSE(CdcIonGetFd)
REFUSE(CdcIonGetMemType)
REFUSE(CdcIonGetPhyAdr)
