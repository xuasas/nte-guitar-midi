using System;
using System.IO;
using System.IO.Compression;
using System.Reflection;
using System.Diagnostics;
using System.Windows.Forms;
using System.Drawing;
using System.Runtime.InteropServices;

internal static class Installer {
    [STAThread]
    private static void Main() {
        Application.EnableVisualStyles();
        Application.SetCompatibleTextRenderingDefault(false);
        string defaultParent = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Programs");
        string destination = Path.Combine(defaultParent, "NTE Guitar MIDI");
        using (var form = new Form { Text="明音凛的旋律工坊 安装程序", ClientSize=new Size(590,230), StartPosition=FormStartPosition.CenterScreen, FormBorderStyle=FormBorderStyle.FixedDialog, MaximizeBox=false }) {
            var heading = new Label { Text="安装 明音凛的旋律工坊", Font=new Font("Microsoft YaHei UI",14,FontStyle.Bold), AutoSize=true, Location=new Point(24,18) };
            var info = new Label { Text="选择新的安装目录。AI 扒谱模型将在首次使用时联网下载。", AutoSize=true, Location=new Point(27,59) };
            var pathBox = new TextBox { Text=destination, Location=new Point(27,94), Size=new Size(420,30) };
            var browse = new Button { Text="浏览…", Location=new Point(455,92), Size=new Size(105,29) };
            var install = new Button { Text="安装", Location=new Point(350,165), Size=new Size(100,34) };
            var cancel = new Button { Text="取消", Location=new Point(460,165), Size=new Size(100,34), DialogResult=DialogResult.Cancel };
            browse.Click += delegate { using (var dialog = new FolderBrowserDialog { Description="选择安装位置的上一级目录" }) if (dialog.ShowDialog(form)==DialogResult.OK) pathBox.Text=Path.Combine(dialog.SelectedPath,"NTE Guitar MIDI"); };
            cancel.Click += delegate { form.Close(); };
            install.Click += delegate {
                try {
                    string root=Path.GetFullPath(pathBox.Text.Trim());
                    if (String.IsNullOrWhiteSpace(pathBox.Text) || !Path.IsPathRooted(pathBox.Text) || root == Path.GetPathRoot(root)) throw new InvalidOperationException("请选择有效的安装目录。");
                    if (Directory.Exists(root) && Directory.GetFileSystemEntries(root).Length != 0) throw new InvalidOperationException("安装目录不为空，请选择一个新目录，避免覆盖已有文件。");
                    Directory.CreateDirectory(root);
                    using (Stream payload=Assembly.GetExecutingAssembly().GetManifestResourceStream("payload.zip"))
                    using (var archive=new ZipArchive(payload, ZipArchiveMode.Read)) {
                        foreach (var entry in archive.Entries) {
                            string target=Path.GetFullPath(Path.Combine(root,entry.FullName.Replace('/',Path.DirectorySeparatorChar)));
                            if (!target.StartsWith(root.TrimEnd(Path.DirectorySeparatorChar)+Path.DirectorySeparatorChar,StringComparison.OrdinalIgnoreCase)) throw new InvalidDataException("安装包包含无效路径。");
                            if (String.IsNullOrEmpty(entry.Name)) { Directory.CreateDirectory(target); continue; }
                            Directory.CreateDirectory(Path.GetDirectoryName(target));
                            using (var input=entry.Open()) using (var output=new FileStream(target,FileMode.CreateNew,FileAccess.Write,FileShare.None)) input.CopyTo(output);
                        }
                    }
                    string exe=Path.Combine(root,"明音凛的旋律工坊.exe");
                    if (!File.Exists(exe)) throw new FileNotFoundException("安装包缺少程序文件。",exe);
                    try {
                        Type type=Type.GetTypeFromProgID("WScript.Shell");
                        dynamic shell=Activator.CreateInstance(type);
                        string desktop=Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory);
                        string shortcutPath=Path.Combine(desktop,"明音凛的旋律工坊.lnk");
                        if (!File.Exists(shortcutPath)) { dynamic shortcut=shell.CreateShortcut(shortcutPath); shortcut.TargetPath=exe; shortcut.WorkingDirectory=root; shortcut.Save(); }
                    } catch { }
                    if (MessageBox.Show(form,"安装完成。现在打开软件吗？\n\n首次扒谱时，请在软件中安装模型环境。","安装完成",MessageBoxButtons.YesNo,MessageBoxIcon.Information)==DialogResult.Yes) Process.Start(new ProcessStartInfo(exe) { WorkingDirectory=root, UseShellExecute=true });
                    form.Close();
                } catch(Exception e) { MessageBox.Show(form,"安装未完成："+e.Message+"\n\n请检查目录权限，或选择一个新的空目录。","安装失败",MessageBoxButtons.OK,MessageBoxIcon.Error); }
            };
            form.Controls.AddRange(new Control[] {heading,info,pathBox,browse,install,cancel}); form.CancelButton=cancel; Application.Run(form);
        }
    }
}