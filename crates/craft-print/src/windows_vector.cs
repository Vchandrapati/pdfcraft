// Vector printing on Windows: compiled by windows_print.ps1 (Add-Type) from CRAFT_PRINT_VECTOR_SOURCE.
// .NET Framework's C# 5 compiler builds it, so no newer syntax.
using System;
using System.Runtime.InteropServices;

public class CraftVectorSetupException : Exception {
    public CraftVectorSetupException(string message) : base(message) {}
}

public sealed class CraftVectorJob : IDisposable {
    [StructLayout(LayoutKind.Sequential)] struct SizeF { public float W, H; }
    [StructLayout(LayoutKind.Sequential)] struct Matrix { public float M11, M12, M21, M22, Dx, Dy; }
    [StructLayout(LayoutKind.Sequential)] struct PrintControlProperties { public int FontSubset; public float RasterDpi; public int ColorSpace; }

    [DllImport("d3d11.dll")]
    static extern int D3D11CreateDevice(IntPtr adapter, int driverType, IntPtr software, uint flags, IntPtr levels, uint count, uint sdk, out IntPtr device, out int level, out IntPtr context);
    [DllImport("d2d1.dll")]
    static extern int D2D1CreateDevice(IntPtr dxgiDevice, IntPtr properties, out IntPtr device);
    [DllImport("Windows.Data.Pdf.dll")]
    static extern int PdfCreateRenderer(IntPtr dxgiDevice, out IntPtr renderer);
    [DllImport("shlwapi.dll")]
    static extern IntPtr SHCreateMemStream(byte[] data, uint size);
    [DllImport("shlwapi.dll", CharSet = CharSet.Unicode)]
    static extern int SHCreateStreamOnFileEx(string file, uint mode, uint attributes, bool create, IntPtr template, out IntPtr stream);
    [DllImport("ole32.dll")]
    static extern int CoCreateInstance(ref Guid clsid, IntPtr outer, uint context, ref Guid iid, out IntPtr instance);

    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int CreateDeviceContextFn(IntPtr self, int options, out IntPtr context);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int CreatePrintControlFn(IntPtr self, IntPtr wic, IntPtr target, ref PrintControlProperties properties, out IntPtr control);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int CreateCommandListFn(IntPtr self, out IntPtr list);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate void SetTargetFn(IntPtr self, IntPtr image);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate void SetTransformFn(IntPtr self, ref Matrix transform);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate void BeginDrawFn(IntPtr self);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int EndDrawFn(IntPtr self, IntPtr tag1, IntPtr tag2);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int RenderPageFn(IntPtr self, IntPtr page, IntPtr context, IntPtr parameters);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int CloseFn(IntPtr self);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int AddPageFn(IntPtr self, IntPtr list, SizeF size, IntPtr ticket, IntPtr tag1, IntPtr tag2);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int CreateTargetFn(IntPtr self, [MarshalAs(UnmanagedType.LPWStr)] string printer, [MarshalAs(UnmanagedType.LPWStr)] string job, IntPtr output, IntPtr ticket, out IntPtr target);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)] delegate int CancelFn(IntPtr self);

    // ID2D1Device
    const int CreateDeviceContext = 4, CreatePrintControl = 5;
    // ID2D1DeviceContext (ID2D1RenderTarget's slots first)
    const int SetTransform = 30, BeginDraw = 48, EndDraw = 49, CreateCommandList = 67, SetTarget = 74;
    // ID2D1CommandList, ID2D1PrintControl, IPdfRendererNative, IPrintDocumentPackageTarget(Factory)
    const int ListClose = 5, AddPage = 3, ControlClose = 4, RenderPageToDeviceContext = 4, CreateTarget = 3, Cancel = 5;

    IntPtr d3d, dxgi, d2d, dc, renderer, wic, factory, target, control, output;
    bool closed;

    static T Slot<T>(IntPtr self, int index) where T : class {
        IntPtr method = Marshal.ReadIntPtr(Marshal.ReadIntPtr(self), index * IntPtr.Size);
        return (T)(object)Marshal.GetDelegateForFunctionPointer(method, typeof(T));
    }
    static void Check(int hr, string what) {
        if (hr < 0) throw new COMException(what + " failed (0x" + hr.ToString("X8") + ")", hr);
    }
    static void Release(ref IntPtr p) {
        if (p != IntPtr.Zero) { Marshal.Release(p); p = IntPtr.Zero; }
    }

    // No printer (null or empty: PowerShell passes $null as "") prepares the renderer only: sheets are drawn into command lists and discarded
    // (the dry run). outputFile, when not empty, receives the job's output instead of the printer's port.
    public static CraftVectorJob Start(string printer, string title, byte[] jobTicket, string outputFile, float rasterDpi) {
        CraftVectorJob job = new CraftVectorJob();
        try {
            job.Setup(printer, title, jobTicket, outputFile, rasterDpi);
        } catch (Exception e) {
            job.Dispose();
            throw new CraftVectorSetupException(e.Message);
        }
        return job;
    }

    void Setup(string printer, string title, byte[] jobTicket, string outputFile, float rasterDpi) {
        int level;
        IntPtr immediate;
        // Hardware first, then WARP, the software device every Windows has. BGRA support is what Direct2D needs.
        int hr = D3D11CreateDevice(IntPtr.Zero, 1, IntPtr.Zero, 0x20, IntPtr.Zero, 0, 7, out d3d, out level, out immediate);
        if (hr < 0) hr = D3D11CreateDevice(IntPtr.Zero, 5, IntPtr.Zero, 0x20, IntPtr.Zero, 0, 7, out d3d, out level, out immediate);
        Check(hr, "creating a Direct3D device");
        Release(ref immediate);
        Guid dxgiId = new Guid("54ec77fa-1377-44e6-8c32-88fd5f44c84c");
        Check(Marshal.QueryInterface(d3d, ref dxgiId, out dxgi), "finding the DXGI device");
        Check(D2D1CreateDevice(dxgi, IntPtr.Zero, out d2d), "creating a Direct2D device");
        Check(Slot<CreateDeviceContextFn>(d2d, CreateDeviceContext)(d2d, 0, out dc), "creating a Direct2D context");
        Check(PdfCreateRenderer(dxgi, out renderer), "creating the PDF renderer");
        if (string.IsNullOrEmpty(printer)) return;

        Guid wicClass = new Guid("cacaf262-9370-4615-a13b-9f5539da4c0a"), wicId = new Guid("ec5ec8a9-c395-4314-9c77-54d7a935ff70");
        Check(CoCreateInstance(ref wicClass, IntPtr.Zero, 1, ref wicId, out wic), "creating the imaging factory");
        Guid factoryClass = new Guid("348ef17d-6c81-4982-92b4-ee188a43867a"), factoryId = new Guid("d2959bf7-b31b-4a3d-9600-712eb1335ba4");
        Check(CoCreateInstance(ref factoryClass, IntPtr.Zero, 1, ref factoryId, out factory), "creating the print job factory");
        if (!string.IsNullOrEmpty(outputFile)) {
            // STGM_CREATE | STGM_WRITE
            Check(SHCreateStreamOnFileEx(outputFile, 0x1001, 0x80, true, IntPtr.Zero, out output), "creating the output file");
        }
        IntPtr ticket = SHCreateMemStream(jobTicket, (uint)jobTicket.Length);
        if (ticket == IntPtr.Zero) throw new OutOfMemoryException("the print ticket could not be prepared");
        try {
            Check(Slot<CreateTargetFn>(factory, CreateTarget)(factory, printer, title, output, ticket, out target), "starting the print job");
        } finally {
            Release(ref ticket);
        }
        // Subset fonts (default), sRGB.
        PrintControlProperties properties = new PrintControlProperties { FontSubset = 0, RasterDpi = rasterDpi, ColorSpace = 1 };
        Check(Slot<CreatePrintControlFn>(d2d, CreatePrintControl)(d2d, wic, target, ref properties, out control), "creating the Direct2D print control");
    }

    // Draw one PDF page (a Windows.Data.Pdf.PdfPage) as a sheet of width x height DIPs, scaled by
    // scale and moved by (dx, dy) DIPs; pageTicket is the sheet's own print ticket.
    public void AddSheet(object page, float width, float height, byte[] pageTicket, float scale, float dx, float dy) {
        IntPtr list = IntPtr.Zero, unknown = IntPtr.Zero, ticket = IntPtr.Zero;
        try {
            Check(Slot<CreateCommandListFn>(dc, CreateCommandList)(dc, out list), "creating a command list");
            Slot<SetTargetFn>(dc, SetTarget)(dc, list);
            Slot<BeginDrawFn>(dc, BeginDraw)(dc);
            Matrix transform = new Matrix { M11 = scale, M22 = scale, Dx = dx, Dy = dy };
            Slot<SetTransformFn>(dc, SetTransform)(dc, ref transform);
            unknown = Marshal.GetIUnknownForObject(page);
            int rendered = Slot<RenderPageFn>(renderer, RenderPageToDeviceContext)(renderer, unknown, dc, IntPtr.Zero);
            int drawn = Slot<EndDrawFn>(dc, EndDraw)(dc, IntPtr.Zero, IntPtr.Zero);
            Slot<SetTargetFn>(dc, SetTarget)(dc, IntPtr.Zero);
            Check(rendered, "rendering the sheet");
            Check(drawn, "drawing the sheet");
            Check(Slot<CloseFn>(list, ListClose)(list), "finishing the sheet");
            if (control == IntPtr.Zero) return;
            if (pageTicket != null) {
                ticket = SHCreateMemStream(pageTicket, (uint)pageTicket.Length);
                if (ticket == IntPtr.Zero) throw new OutOfMemoryException("the sheet's print ticket could not be prepared");
            }
            SizeF size = new SizeF { W = width, H = height };
            Check(Slot<AddPageFn>(control, AddPage)(control, list, size, ticket, IntPtr.Zero, IntPtr.Zero), "spooling the sheet");
        } finally {
            Release(ref ticket);
            Release(ref unknown);
            Release(ref list);
        }
    }

    // Finish the document: the spooler has the whole job when this returns.
    public void Close() {
        if (control != IntPtr.Zero) Check(Slot<CloseFn>(control, ControlClose)(control), "finishing the print job");
        closed = true;
    }

    // A job that was not closed is cancelled, so a failure never prints part of the document.
    public void Dispose() {
        if (!closed && target != IntPtr.Zero) Slot<CancelFn>(target, Cancel)(target);
        Release(ref control);
        Release(ref target);
        Release(ref factory);
        Release(ref wic);
        Release(ref renderer);
        Release(ref dc);
        Release(ref d2d);
        Release(ref dxgi);
        Release(ref d3d);
        Release(ref output);
    }
}
