/*
 * Binary interface of the RaplPkg driver, protocol 2.
 *
 * Mirrored by bridge/src/abi.rs (IOCTLs, structures, flags) and by
 * core/src/policy.rs and core/src/rapl.rs (write policy). The bridge test
 * `driver_header_matches_the_rust_contract` parses the #define lines below,
 * so keep one "#define NAME VALUE" per line for every shared constant.
 */
#pragma once

#include <ntddk.h>

#define RAPL_PROTOCOL_VERSION 2u

#define RAPL_DEVICE_TYPE 0x8010u
#define RAPL_IOCTL_BASE 0x800u

/* Fixed register whitelist. */
#define RAPL_MSR_POWER_UNIT 0x606u
#define RAPL_MSR_PKG_POWER_LIMIT 0x610u
#define RAPL_MMIO_PKG_POWER_LIMIT 0x59A0u

/* Supported platform: GenuineIntel, family 6, model 0xB7, one package. */
#define RAPL_VENDOR_INTEL 0x8086u
#define RAPL_SUPPORTED_FAMILY 0x6u
#define RAPL_SUPPORTED_MODEL 0xB7u

/* Register-level write policy (check_register_write in core/src/policy.rs). */
#define RAPL_MIN_WATTS 5u
#define RAPL_MAX_TURBO_WATTS 157u
#define RAPL_POWER_FIELD_MASK 0x0000FFFF0000FFFFull
#define RAPL_LOCK_BIT 0x8000000000000000ull
#define RAPL_PL1_ENABLE 0x0000000000008000ull
#define RAPL_PL2_ENABLE 0x0000800000000000ull
#define RAPL_LIMIT_MASK 0x7FFFull
#define RAPL_POWER_UNIT_MASK 0xFull

/* RAPL_INFO.Flags */
#define RAPL_FLAG_MMIO_VERIFIED 0x1u
#define RAPL_FLAG_READ_ONLY 0x2u
#define RAPL_FLAG_CPU_SUPPORTED 0x4u
#define RAPL_FLAG_MMIO_READABLE 0x8u

typedef enum _RAPL_REGISTER_ID {
    RaplRegisterUnits = 1,
    RaplRegisterPackageLimit = 2,
    RaplRegisterMmioPackageLimit = 3
} RAPL_REGISTER_ID;

typedef struct _RAPL_REQUEST {
    ULONG Size;
    ULONG Version;
    ULONG RegisterId;
    ULONG Reserved;
    /* Writes only: the driver refuses when the register no longer holds this value. */
    ULONGLONG ExpectedValue;
    ULONGLONG Value;
} RAPL_REQUEST;

typedef struct _RAPL_RESPONSE {
    ULONG Size;
    ULONG Version;
    ULONG RegisterId;
    ULONG Flags;
    /* Register value; for writes the readback after the write. */
    ULONGLONG Value;
} RAPL_RESPONSE;

typedef struct _RAPL_INFO {
    ULONG Size;
    ULONG Version;
    ULONG Vendor;
    ULONG Family;
    ULONG Model;
    ULONG Stepping;
    ULONG Flags;
    ULONG PackageCount;
    /* Physical MCHBAR base, or 0 when MMIO reads are disabled. */
    ULONGLONG MchbarBase;
} RAPL_INFO;

C_ASSERT(sizeof(RAPL_REQUEST) == 32);
C_ASSERT(sizeof(RAPL_RESPONSE) == 24);
C_ASSERT(sizeof(RAPL_INFO) == 40);
C_ASSERT(FIELD_OFFSET(RAPL_REQUEST, ExpectedValue) == 16);
C_ASSERT(FIELD_OFFSET(RAPL_RESPONSE, Value) == 16);
C_ASSERT(FIELD_OFFSET(RAPL_INFO, MchbarBase) == 32);

#define IOCTL_RAPL_GET_INFO \
    CTL_CODE(RAPL_DEVICE_TYPE, RAPL_IOCTL_BASE + 0, METHOD_BUFFERED, FILE_READ_DATA)
#define IOCTL_RAPL_READ_MSR \
    CTL_CODE(RAPL_DEVICE_TYPE, RAPL_IOCTL_BASE + 1, METHOD_BUFFERED, FILE_READ_DATA)
#define IOCTL_RAPL_READ_MMIO \
    CTL_CODE(RAPL_DEVICE_TYPE, RAPL_IOCTL_BASE + 2, METHOD_BUFFERED, FILE_READ_DATA)
#define IOCTL_RAPL_WRITE_MSR \
    CTL_CODE(RAPL_DEVICE_TYPE, RAPL_IOCTL_BASE + 3, METHOD_BUFFERED, FILE_WRITE_DATA)
#define IOCTL_RAPL_WRITE_MMIO \
    CTL_CODE(RAPL_DEVICE_TYPE, RAPL_IOCTL_BASE + 4, METHOD_BUFFERED, FILE_WRITE_DATA)

C_ASSERT(IOCTL_RAPL_GET_INFO == 0x80106000u);
C_ASSERT(IOCTL_RAPL_READ_MSR == 0x80106004u);
C_ASSERT(IOCTL_RAPL_READ_MMIO == 0x80106008u);
C_ASSERT(IOCTL_RAPL_WRITE_MSR == 0x8010A00Cu);
C_ASSERT(IOCTL_RAPL_WRITE_MMIO == 0x8010A010u);
