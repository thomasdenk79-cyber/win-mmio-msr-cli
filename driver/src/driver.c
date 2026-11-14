/*
 * rapl-pkg.sys: KMDF non-PnP control device \Device\RaplPkg that gives the
 * rapl-bridge access to the Intel package RAPL power limit of one supported CPU.
 *
 * - Fixed register whitelist (MSR 0x606/0x610, MCHBAR+0x59A0); no caller-supplied
 *   MSR numbers or physical addresses.
 * - Device ACL: SYSTEM and Administrators only, FILE_DEVICE_SECURE_OPEN.
 * - Exact buffer sizes and protocol version; requests are copied before the
 *   response is written because METHOD_BUFFERED shares one system buffer.
 * - Writes are a compare-and-write under the shared write policy; the readback
 *   is returned so that the bridge can audit a mismatch.
 * - Build-time gates (driver/README.md) default to read-only MSR access.
 */
#include <ntddk.h>
#include <wdf.h>
#include <intrin.h>
#include "rapl_ioctl.h"

#ifndef RAPL_ENABLE_MMIO_READ
#define RAPL_ENABLE_MMIO_READ 0
#endif
#ifndef RAPL_MMIO_LAYOUT_VALIDATED
#define RAPL_MMIO_LAYOUT_VALIDATED 0
#endif
#ifndef RAPL_ENABLE_LIVE_WRITES
#define RAPL_ENABLE_LIVE_WRITES 0
#endif
#if RAPL_MMIO_LAYOUT_VALIDATED && !RAPL_ENABLE_MMIO_READ
#error "RAPL_MMIO_LAYOUT_VALIDATED requires RAPL_ENABLE_MMIO_READ"
#endif

#define RAPL_NT_DEVICE_NAME L"\\Device\\RaplPkg"
#define RAPL_DOS_DEVICE_NAME L"\\DosDevices\\RaplPkg"
#define RAPL_DEVICE_SDDL L"D:P(A;;GA;;;SY)(A;;GA;;;BA)"

DRIVER_INITIALIZE DriverEntry;
EVT_WDF_DRIVER_UNLOAD RaplEvtDriverUnload;
EVT_WDF_IO_QUEUE_IO_DEVICE_CONTROL RaplEvtIoDeviceControl;

typedef struct _RAPL_CPU {
    BOOLEAN Intel;
    BOOLEAN Supported;
    ULONG Family;
    ULONG Model;
    ULONG Stepping;
    ULONG Packages;
} RAPL_CPU;

/* Written once in DriverEntry before the device exists; read-only afterwards. */
static RAPL_CPU g_Cpu;

/* Number of processor packages, or 0 when it cannot be determined. */
static ULONG RaplCountPackages(VOID)
{
    ULONG64 buffer[64];
    ULONG length = (ULONG)sizeof(buffer);
    ULONG offset = 0;
    ULONG count = 0;
    NTSTATUS status;

    status = KeQueryLogicalProcessorRelationship(
        NULL, RelationProcessorPackage, (PSYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX)buffer, &length);
    if (!NT_SUCCESS(status)) {
        return 0;
    }
    while (offset < length) {
        PSYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX entry =
            (PSYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX)((PUCHAR)buffer + offset);
        if (length - offset < RTL_SIZEOF_THROUGH_FIELD(SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX, Size) ||
            entry->Size == 0 || entry->Size > length - offset) {
            return 0;
        }
        if (entry->Relationship == RelationProcessorPackage) {
            count++;
        }
        offset += entry->Size;
    }
    return count;
}

static VOID RaplIdentifyCpu(VOID)
{
    int regs[4];
    ULONG signature;
    ULONG family;
    ULONG model;

    __cpuid(regs, 0);
    /* "GenuineIntel" in EBX, EDX, ECX. */
    g_Cpu.Intel = (BOOLEAN)(regs[1] == 0x756E6547 && regs[3] == 0x49656E69 && regs[2] == 0x6C65746E);
    __cpuid(regs, 1);
    signature = (ULONG)regs[0];
    family = (signature >> 8) & 0xF;
    model = (signature >> 4) & 0xF;
    if (family == 0xF) {
        family += (signature >> 20) & 0xFF;
    }
    if (family == 0x6 || family >= 0xF) {
        model |= ((signature >> 16) & 0xF) << 4;
    }
    g_Cpu.Family = family;
    g_Cpu.Model = model;
    g_Cpu.Stepping = signature & 0xF;
    g_Cpu.Packages = RaplCountPackages();
    g_Cpu.Supported = (BOOLEAN)(g_Cpu.Intel && family == RAPL_SUPPORTED_FAMILY &&
                                model == RAPL_SUPPORTED_MODEL && g_Cpu.Packages == 1);
}

/* A #GP (unknown MSR, or an access blocked by the hypervisor) becomes an error. */
static NTSTATUS RaplReadMsr(_In_ ULONG Msr, _Out_ PULONGLONG Value)
{
    *Value = 0;
    __try {
        *Value = __readmsr(Msr);
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return STATUS_IO_DEVICE_ERROR;
    }
    return STATUS_SUCCESS;
}

#if RAPL_ENABLE_MMIO_READ
/* Host bridge 0:0.0 MCHBAR register: base in bits 41:17, enable in bit 0
 * (12th/13th generation Intel Core datasheet, volume 2). */
#define RAPL_PCI_MCHBAR_OFFSET 0x48u
#define RAPL_MCHBAR_ENABLE 0x1ull
#define RAPL_MCHBAR_BASE_MASK 0x000003FFFFFE0000ull

static NTSTATUS RaplFindMchbar(_Out_ PULONGLONG Base)
{
    USHORT vendor = 0;
    ULONGLONG mchbar = 0;
    ULONG read;

    *Base = 0;
    /* A control device has no PCI PDO, so the HAL is the only configuration reader. */
#pragma warning(push)
#pragma warning(disable : 4996)
    read = HalGetBusDataByOffset(PCIConfiguration, 0, 0, &vendor, 0, sizeof(vendor));
    if (read != sizeof(vendor) || vendor != RAPL_VENDOR_INTEL) {
        return STATUS_NOT_SUPPORTED;
    }
    read = HalGetBusDataByOffset(PCIConfiguration, 0, 0, &mchbar, RAPL_PCI_MCHBAR_OFFSET, sizeof(mchbar));
#pragma warning(pop)
    if (read != sizeof(mchbar) || (mchbar & RAPL_MCHBAR_ENABLE) == 0 ||
        (mchbar & RAPL_MCHBAR_BASE_MASK) == 0) {
        return STATUS_NOT_SUPPORTED;
    }
    *Base = mchbar & RAPL_MCHBAR_BASE_MASK;
    return STATUS_SUCCESS;
}

/* Maps MCHBAR+0x59A0 for one access and unmaps it again. */
static NTSTATUS RaplAccessMmio(_In_ BOOLEAN Write, _In_ ULONGLONG Value, _Out_ PULONGLONG Readback)
{
    PHYSICAL_ADDRESS address;
    volatile ULONG64* reg;
    ULONGLONG base;
    NTSTATUS status;

    *Readback = 0;
    status = RaplFindMchbar(&base);
    if (!NT_SUCCESS(status)) {
        return status;
    }
    address.QuadPart = (LONGLONG)(base + RAPL_MMIO_PKG_POWER_LIMIT);
    reg = (volatile ULONG64*)MmMapIoSpaceEx(
        address, sizeof(ULONG64), (Write ? PAGE_READWRITE : PAGE_READONLY) | PAGE_NOCACHE);
    if (reg == NULL) {
        return STATUS_INSUFFICIENT_RESOURCES;
    }
    if (Write) {
        WRITE_REGISTER_ULONG64(reg, Value);
    }
    *Readback = READ_REGISTER_ULONG64(reg);
    MmUnmapIoSpace((PVOID)reg, sizeof(ULONG64));
    return STATUS_SUCCESS;
}
#endif

static NTSTATUS RaplReadRegister(_In_ ULONG RegisterId, _Out_ PULONGLONG Value)
{
    *Value = 0;
    switch (RegisterId) {
    case RaplRegisterUnits:
        return RaplReadMsr(RAPL_MSR_POWER_UNIT, Value);
    case RaplRegisterPackageLimit:
        return RaplReadMsr(RAPL_MSR_PKG_POWER_LIMIT, Value);
    case RaplRegisterMmioPackageLimit:
#if RAPL_ENABLE_MMIO_READ
        return RaplAccessMmio(FALSE, 0, Value);
#else
        return STATUS_NOT_SUPPORTED;
#endif
    default:
        return STATUS_INVALID_PARAMETER;
    }
}

#if RAPL_ENABLE_LIVE_WRITES
static NTSTATUS RaplWriteMsr(_In_ ULONG Msr, _In_ ULONGLONG Value)
{
    __try {
        __writemsr(Msr, Value);
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return STATUS_IO_DEVICE_ERROR;
    }
    return STATUS_SUCCESS;
}

/*
 * check_register_write in core/src/policy.rs without floating point:
 * watts = field / 2^unit, so "field >= MIN << unit" means "watts >= MIN".
 */
static BOOLEAN RaplWriteAllowed(_In_ ULONGLONG Current, _In_ ULONGLONG Requested, _In_ ULONGLONG Units)
{
    ULONG shift = (ULONG)(Units & RAPL_POWER_UNIT_MASK);
    ULONGLONG pl1 = Requested & RAPL_LIMIT_MASK;
    ULONGLONG pl2 = (Requested >> 32) & RAPL_LIMIT_MASK;

    if ((Current & RAPL_LOCK_BIT) != 0) {
        return FALSE;
    }
    if (((Current ^ Requested) & ~RAPL_POWER_FIELD_MASK) != 0) {
        return FALSE;
    }
    if ((Requested & RAPL_PL1_ENABLE) == 0 || (Requested & RAPL_PL2_ENABLE) == 0) {
        return FALSE;
    }
    return (BOOLEAN)(pl1 >= ((ULONGLONG)RAPL_MIN_WATTS << shift) && pl1 <= pl2 &&
                     pl2 <= ((ULONGLONG)RAPL_MAX_TURBO_WATTS << shift));
}

static NTSTATUS RaplWriteRegister(_In_ ULONG RegisterId, _In_ ULONGLONG Value)
{
    switch (RegisterId) {
    case RaplRegisterPackageLimit:
        return RaplWriteMsr(RAPL_MSR_PKG_POWER_LIMIT, Value);
#if RAPL_MMIO_LAYOUT_VALIDATED
    case RaplRegisterMmioPackageLimit: {
        ULONGLONG ignored;
        return RaplAccessMmio(TRUE, Value, &ignored);
    }
#endif
    default:
        return STATUS_NOT_SUPPORTED;
    }
}
#endif

/* Writes Request->Value only while the register still holds Request->ExpectedValue. */
static NTSTATUS RaplCompareAndWrite(_In_ const RAPL_REQUEST* Request, _Out_ PULONGLONG Readback)
{
#if RAPL_ENABLE_LIVE_WRITES
    ULONGLONG units;
    ULONGLONG current;
    NTSTATUS status;

    *Readback = 0;
    status = RaplReadMsr(RAPL_MSR_POWER_UNIT, &units);
    if (!NT_SUCCESS(status)) {
        return status;
    }
    status = RaplReadRegister(Request->RegisterId, &current);
    if (!NT_SUCCESS(status)) {
        return status;
    }
    if (current != Request->ExpectedValue) {
        return STATUS_RETRY;
    }
    if (!RaplWriteAllowed(current, Request->Value, units)) {
        return STATUS_INVALID_PARAMETER;
    }
    status = RaplWriteRegister(Request->RegisterId, Request->Value);
    if (!NT_SUCCESS(status)) {
        return status;
    }
    return RaplReadRegister(Request->RegisterId, Readback);
#else
    UNREFERENCED_PARAMETER(Request);
    *Readback = 0;
    return STATUS_NOT_SUPPORTED;
#endif
}

static ULONG RaplInfoFlags(_Out_ PULONGLONG MchbarBase)
{
    ULONG flags = RAPL_FLAG_CPU_SUPPORTED;

    *MchbarBase = 0;
    if (!g_Cpu.Supported) {
        return RAPL_FLAG_READ_ONLY;
    }
#if !RAPL_ENABLE_LIVE_WRITES
    flags |= RAPL_FLAG_READ_ONLY;
#endif
#if RAPL_ENABLE_MMIO_READ
    if (NT_SUCCESS(RaplFindMchbar(MchbarBase))) {
        flags |= RAPL_FLAG_MMIO_READABLE;
#if RAPL_MMIO_LAYOUT_VALIDATED
        flags |= RAPL_FLAG_MMIO_VERIFIED;
#endif
    }
#endif
    return flags;
}

static NTSTATUS RaplHandleInfo(
    _In_ WDFREQUEST Request, _In_ size_t InputLength, _In_ size_t OutputLength, _Out_ PULONG_PTR Information)
{
    RAPL_INFO info;
    PVOID buffer;
    NTSTATUS status;

    *Information = 0;
    if (InputLength != 0) {
        return STATUS_INVALID_PARAMETER;
    }
    if (OutputLength != sizeof(RAPL_INFO)) {
        return STATUS_REVISION_MISMATCH;
    }
    status = WdfRequestRetrieveOutputBuffer(Request, sizeof(RAPL_INFO), &buffer, NULL);
    if (!NT_SUCCESS(status)) {
        return status;
    }
    RtlZeroMemory(&info, sizeof(info));
    info.Size = (ULONG)sizeof(info);
    info.Version = RAPL_PROTOCOL_VERSION;
    info.Vendor = g_Cpu.Intel ? RAPL_VENDOR_INTEL : 0;
    info.Family = g_Cpu.Family;
    info.Model = g_Cpu.Model;
    info.Stepping = g_Cpu.Stepping;
    info.PackageCount = g_Cpu.Packages;
    info.Flags = RaplInfoFlags(&info.MchbarBase);
    RtlCopyMemory(buffer, &info, sizeof(info));
    *Information = sizeof(info);
    return STATUS_SUCCESS;
}

static BOOLEAN RaplRegisterMatchesIoctl(_In_ ULONG IoControlCode, _In_ ULONG RegisterId)
{
    switch (IoControlCode) {
    case IOCTL_RAPL_READ_MSR:
        return (BOOLEAN)(RegisterId == RaplRegisterUnits || RegisterId == RaplRegisterPackageLimit);
    case IOCTL_RAPL_WRITE_MSR:
        return (BOOLEAN)(RegisterId == RaplRegisterPackageLimit);
    case IOCTL_RAPL_READ_MMIO:
    case IOCTL_RAPL_WRITE_MMIO:
        return (BOOLEAN)(RegisterId == RaplRegisterMmioPackageLimit);
    default:
        return FALSE;
    }
}

static NTSTATUS RaplHandleRegister(
    _In_ WDFREQUEST Request,
    _In_ ULONG IoControlCode,
    _In_ size_t InputLength,
    _In_ size_t OutputLength,
    _Out_ PULONG_PTR Information)
{
    RAPL_REQUEST input;
    RAPL_RESPONSE response;
    PVOID inBuffer;
    PVOID outBuffer;
    ULONGLONG value = 0;
    BOOLEAN write = (BOOLEAN)(IoControlCode == IOCTL_RAPL_WRITE_MSR || IoControlCode == IOCTL_RAPL_WRITE_MMIO);
    NTSTATUS status;

    *Information = 0;
    if (InputLength != sizeof(RAPL_REQUEST) || OutputLength != sizeof(RAPL_RESPONSE)) {
        return STATUS_REVISION_MISMATCH;
    }
    status = WdfRequestRetrieveInputBuffer(Request, sizeof(RAPL_REQUEST), &inBuffer, NULL);
    if (!NT_SUCCESS(status)) {
        return status;
    }
    status = WdfRequestRetrieveOutputBuffer(Request, sizeof(RAPL_RESPONSE), &outBuffer, NULL);
    if (!NT_SUCCESS(status)) {
        return status;
    }
    /* METHOD_BUFFERED: input and output share one system buffer. */
    RtlCopyMemory(&input, inBuffer, sizeof(input));
    if (input.Size != sizeof(RAPL_REQUEST) || input.Version != RAPL_PROTOCOL_VERSION) {
        return STATUS_REVISION_MISMATCH;
    }
    if (input.Reserved != 0 || !RaplRegisterMatchesIoctl(IoControlCode, input.RegisterId)) {
        return STATUS_INVALID_PARAMETER;
    }
    if (!write && (input.ExpectedValue != 0 || input.Value != 0)) {
        return STATUS_INVALID_PARAMETER;
    }
    if (!g_Cpu.Supported) {
        return STATUS_NOT_SUPPORTED;
    }
    status = write ? RaplCompareAndWrite(&input, &value) : RaplReadRegister(input.RegisterId, &value);
    if (!NT_SUCCESS(status)) {
        return status;
    }
    RtlZeroMemory(&response, sizeof(response));
    response.Size = (ULONG)sizeof(response);
    response.Version = RAPL_PROTOCOL_VERSION;
    response.RegisterId = input.RegisterId;
    response.Value = value;
    RtlCopyMemory(outBuffer, &response, sizeof(response));
    *Information = sizeof(response);
    return STATUS_SUCCESS;
}

_Use_decl_annotations_
VOID RaplEvtIoDeviceControl(
    WDFQUEUE Queue, WDFREQUEST Request, size_t OutputBufferLength, size_t InputBufferLength, ULONG IoControlCode)
{
    ULONG_PTR information = 0;
    NTSTATUS status;

    UNREFERENCED_PARAMETER(Queue);
    switch (IoControlCode) {
    case IOCTL_RAPL_GET_INFO:
        status = RaplHandleInfo(Request, InputBufferLength, OutputBufferLength, &information);
        break;
    case IOCTL_RAPL_READ_MSR:
    case IOCTL_RAPL_READ_MMIO:
    case IOCTL_RAPL_WRITE_MSR:
    case IOCTL_RAPL_WRITE_MMIO:
        status = RaplHandleRegister(Request, IoControlCode, InputBufferLength, OutputBufferLength, &information);
        break;
    default:
        status = STATUS_INVALID_DEVICE_REQUEST;
        break;
    }
    WdfRequestCompleteWithInformation(Request, status, information);
}

static NTSTATUS RaplCreateControlDevice(_In_ WDFDRIVER Driver)
{
    DECLARE_CONST_UNICODE_STRING(sddl, RAPL_DEVICE_SDDL);
    DECLARE_CONST_UNICODE_STRING(deviceName, RAPL_NT_DEVICE_NAME);
    DECLARE_CONST_UNICODE_STRING(linkName, RAPL_DOS_DEVICE_NAME);
    PWDFDEVICE_INIT init;
    WDFDEVICE device;
    WDF_IO_QUEUE_CONFIG queueConfig;
    WDF_OBJECT_ATTRIBUTES queueAttributes;
    NTSTATUS status;

    init = WdfControlDeviceInitAllocate(Driver, &sddl);
    if (init == NULL) {
        return STATUS_INSUFFICIENT_RESOURCES;
    }
    WdfDeviceInitSetDeviceType(init, RAPL_DEVICE_TYPE);
    WdfDeviceInitSetCharacteristics(init, FILE_DEVICE_SECURE_OPEN, TRUE);
    WdfDeviceInitSetIoType(init, WdfDeviceIoBuffered);
    status = WdfDeviceInitAssignName(init, &deviceName);
    if (NT_SUCCESS(status)) {
        status = WdfDeviceCreate(&init, WDF_NO_OBJECT_ATTRIBUTES, &device);
    }
    if (!NT_SUCCESS(status)) {
        if (init != NULL) {
            WdfDeviceInitFree(init);
        }
        return status;
    }

    status = WdfDeviceCreateSymbolicLink(device, &linkName);
    if (NT_SUCCESS(status)) {
        WDF_IO_QUEUE_CONFIG_INIT_DEFAULT_QUEUE(&queueConfig, WdfIoQueueDispatchSequential);
        queueConfig.EvtIoDeviceControl = RaplEvtIoDeviceControl;
        queueConfig.PowerManaged = WdfFalse;
        WDF_OBJECT_ATTRIBUTES_INIT(&queueAttributes);
        queueAttributes.ExecutionLevel = WdfExecutionLevelPassive;
        status = WdfIoQueueCreate(device, &queueConfig, &queueAttributes, WDF_NO_HANDLE);
    }
    if (!NT_SUCCESS(status)) {
        WdfObjectDelete(device);
        return status;
    }
    WdfControlFinishInitializing(device);
    return STATUS_SUCCESS;
}

/* A non-PnP driver must register an unload callback to be stoppable. */
_Use_decl_annotations_
VOID RaplEvtDriverUnload(WDFDRIVER Driver)
{
    UNREFERENCED_PARAMETER(Driver);
}

_Use_decl_annotations_
NTSTATUS DriverEntry(PDRIVER_OBJECT DriverObject, PUNICODE_STRING RegistryPath)
{
    WDF_DRIVER_CONFIG config;
    WDFDRIVER driver;
    NTSTATUS status;

    RaplIdentifyCpu();
    WDF_DRIVER_CONFIG_INIT(&config, WDF_NO_EVENT_CALLBACK);
    config.DriverInitFlags |= WdfDriverInitNonPnpDriver;
    config.EvtDriverUnload = RaplEvtDriverUnload;
    status = WdfDriverCreate(DriverObject, RegistryPath, WDF_NO_OBJECT_ATTRIBUTES, &config, &driver);
    if (!NT_SUCCESS(status)) {
        return status;
    }
    return RaplCreateControlDevice(driver);
}
