using System.Globalization;
using System.Windows;
using System.Windows.Data;
using System.Windows.Input;
using BalanceBot.Core.Analysis;
using BalanceBot.Desktop.ViewModels;
using BalanceBot.Desktop.Views;
using BalanceBot.Protocol.Dtos;
using ScottPlot;

namespace BalanceBot.Desktop;

public partial class MainWindow : Window
{
    private readonly MainViewModel _vm;

    public MainWindow()
    {
        InitializeComponent();

        _vm = new MainViewModel();
        DataContext = _vm;

        Loaded += OnLoaded;
        KeyDown += OnWindowKeyDown;
        KeyUp += OnWindowKeyUp;
    }

    private void OnLoaded(object sender, RoutedEventArgs e)
    {
        // Даём UI время отрисовать вкладки, затем настраиваем графики
        Dispatcher.BeginInvoke(() =>
        {
            SetupPlots();
            SetupBlackBoxPlotTheme();
            SetupBodePlotsTheme();
        }, System.Windows.Threading.DispatcherPriority.Render);
    }

    private DashboardView? GetDashboardView()
    {
        // Находим DashboardView в визуальном дереве
        return FindVisualChild<DashboardView>(this);
    }

    private BlackBoxView? GetBlackBoxView()
    {
        return FindVisualChild<BlackBoxView>(this);
    }

    private BodeView? GetBodeView()
    {
        return FindVisualChild<BodeView>(this);
    }

    private static T? FindVisualChild<T>(DependencyObject parent) where T : DependencyObject
    {
        for (int i = 0; i < System.Windows.Media.VisualTreeHelper.GetChildrenCount(parent); i++)
        {
            var child = System.Windows.Media.VisualTreeHelper.GetChild(parent, i);
            if (child is T typedChild)
                return typedChild;

            var result = FindVisualChild<T>(child);
            if (result != null)
                return result;
        }
        return null;
    }

    private void SetupPlots()
    {
        var dashboard = GetDashboardView();
        if (dashboard == null) return;

        var plotOri = dashboard.PlotOrientationControl.Plot;
        plotOri.FigureBackground.Color = Color.FromHex("#0D0F14");
        plotOri.DataBackground.Color = Color.FromHex("#161A22");
        plotOri.Axes.Color(Color.FromHex("#8B95A5"));
        plotOri.Grid.MajorLineColor = Color.FromHex("#1E232E");
        plotOri.Axes.Title.Label.Text = "Ориентация и IMU (Градусы °)";
        plotOri.Axes.Title.Label.FontSize = 13;

        var angleStreamer = plotOri.Add.DataStreamer(600);
        angleStreamer.Color = Color.FromHex("#4A90FF");
        angleStreamer.ViewScrollLeft();

        var gyroStreamer = plotOri.Add.DataStreamer(600);
        gyroStreamer.Color = Color.FromHex("#00D084");
        gyroStreamer.ViewScrollLeft();

        var trimStreamer = plotOri.Add.DataStreamer(600);
        trimStreamer.Color = Color.FromHex("#E8ECF1");
        trimStreamer.ViewScrollLeft();

        var accelStreamer = plotOri.Add.DataStreamer(600);
        accelStreamer.Color = Color.FromHex("#80D8FF");
        accelStreamer.IsVisible = false;
        accelStreamer.ViewScrollLeft();

        var leanStreamer = plotOri.Add.DataStreamer(600);
        leanStreamer.Color = Color.FromHex("#FF6D00");
        leanStreamer.IsVisible = false;
        leanStreamer.ViewScrollLeft();

        var plotMot = dashboard.PlotMotorsControl.Plot;
        plotMot.FigureBackground.Color = Color.FromHex("#0D0F14");
        plotMot.DataBackground.Color = Color.FromHex("#161A22");
        plotMot.Axes.Color(Color.FromHex("#8B95A5"));
        plotMot.Grid.MajorLineColor = Color.FromHex("#1E232E");
        plotMot.Axes.Title.Label.Text = "Привод колес и регулятор (об/с)";
        plotMot.Axes.Title.Label.FontSize = 13;

        var cmdStreamer = plotMot.Add.DataStreamer(600);
        cmdStreamer.Color = Color.FromHex("#FFA502");
        cmdStreamer.ViewScrollLeft();

        var wheelStreamer = plotMot.Add.DataStreamer(600);
        wheelStreamer.Color = Color.FromHex("#B388FF");
        wheelStreamer.ViewScrollLeft();

        var termPStreamer = plotMot.Add.DataStreamer(600);
        termPStreamer.Color = Color.FromHex("#FF4081");
        termPStreamer.IsVisible = false;
        termPStreamer.ViewScrollLeft();

        var termDStreamer = plotMot.Add.DataStreamer(600);
        termDStreamer.Color = Color.FromHex("#78909C");
        termDStreamer.IsVisible = false;
        termDStreamer.ViewScrollLeft();

        _vm.PropertyChanged += (_, args) =>
        {
            if (args.PropertyName == nameof(_vm.Dashboard) && _vm.Dashboard != null)
            {
                var d = _vm.Dashboard;

                d.AngleStreamer = angleStreamer;
                d.GyroStreamer = gyroStreamer;
                d.TrimStreamer = trimStreamer;
                d.AccelAngleStreamer = accelStreamer;
                d.LeanStreamer = leanStreamer;

                d.CmdStreamer = cmdStreamer;
                d.WheelRateStreamer = wheelStreamer;
                d.TermPStreamer = termPStreamer;
                d.TermDStreamer = termDStreamer;

                d.RequestPlotsRefresh = () =>
                {
                    dashboard.PlotOrientationControl.Refresh();
                    dashboard.PlotMotorsControl.Refresh();
                };

                d.RequestPlotsReset = () =>
                {
                    dashboard.PlotOrientationControl.Plot.Axes.AutoScale();
                    dashboard.PlotMotorsControl.Plot.Axes.AutoScale();
                    dashboard.PlotOrientationControl.Refresh();
                    dashboard.PlotMotorsControl.Refresh();
                };
            }

            if (args.PropertyName == nameof(_vm.BlackBox) && _vm.BlackBox != null)
            {
                _vm.BlackBox.OnFramesLoaded = RenderBlackBoxFrames;
            }

            if (args.PropertyName == nameof(_vm.Bode) && _vm.Bode != null)
            {
                _vm.Bode.OnStepResponse = RenderBodeStepResponse;
                _vm.Bode.OnAnalysisComplete = RenderBodeResult;
            }
        };

        dashboard.PlotOrientationControl.Refresh();
        dashboard.PlotMotorsControl.Refresh();
    }

    private void SetupBlackBoxPlotTheme()
    {
        var view = GetBlackBoxView();
        if (view == null) return;

        var plot = view.PlotBlackBoxControl.Plot;
        plot.FigureBackground.Color = Color.FromHex("#0D0F14");
        plot.DataBackground.Color = Color.FromHex("#161A22");
        plot.Axes.Color(Color.FromHex("#8B95A5"));
        plot.Grid.MajorLineColor = Color.FromHex("#1E232E");
        plot.ShowLegend(Edge.Top);
        view.PlotBlackBoxControl.Refresh();
    }

    private void RenderBlackBoxFrames(List<BlackBoxFrameDto> frames)
    {
        var view = GetBlackBoxView();
        if (view == null) return;

        var plot = view.PlotBlackBoxControl.Plot;
        plot.Clear();

        uint startMs = frames[0].TimestampMs;
        double[] times = frames.Select(f => (double)(f.TimestampMs - startMs)).ToArray();
        double[] angles = frames.Select(f => (double)f.Angle).ToArray();
        double[] gyros = frames.Select(f => (double)f.Gyro).ToArray();
        double[] cmds = frames.Select(f => (double)f.Cmd / 3200.0).ToArray();

        var angleScatter = plot.Add.Scatter(times, angles, Color.FromHex("#4A90FF"));
        angleScatter.LegendText = "Угол наклона (°)";

        var gyroScatter = plot.Add.Scatter(times, gyros, Color.FromHex("#00D084"));
        gyroScatter.LegendText = "Гироскоп (°/с)";

        var cmdScatter = plot.Add.Scatter(times, cmds, Color.FromHex("#FFA502"));
        cmdScatter.LegendText = "Команда мотора (об/с)";

        plot.Axes.AutoScale();
        view.PlotBlackBoxControl.Refresh();
    }

    private void SetupBodePlotsTheme()
    {
        var view = GetBodeView();
        if (view == null) return;

        foreach (var wpfPlot in new[] { view.PlotStepResponseControl, view.PlotBodeMagControl, view.PlotBodePhaseControl })
        {
            var plot = wpfPlot.Plot;
            plot.FigureBackground.Color = Color.FromHex("#0D0F14");
            plot.DataBackground.Color = Color.FromHex("#161A22");
            plot.Axes.Color(Color.FromHex("#8B95A5"));
            plot.Grid.MajorLineColor = Color.FromHex("#1E232E");
            wpfPlot.Refresh();
        }

        view.PlotStepResponseControl.Plot.Axes.Title.Label.Text = "Отклик на импульс трима";
        view.PlotStepResponseControl.Plot.Axes.Title.Label.FontSize = 13;

        view.PlotBodeMagControl.Plot.Axes.Title.Label.Text = "АЧХ контура |L| (дБ), ось X — log10(f/Гц)";
        view.PlotBodeMagControl.Plot.Axes.Title.Label.FontSize = 13;

        view.PlotBodePhaseControl.Plot.Axes.Title.Label.Text = "ФЧХ контура ∠L (°), ось X — log10(f/Гц)";
        view.PlotBodePhaseControl.Plot.Axes.Title.Label.FontSize = 13;

        view.PlotStepResponseControl.Plot.ShowLegend(Edge.Top);
        view.PlotBodeMagControl.Plot.ShowLegend(Edge.Top);
        view.PlotBodePhaseControl.Plot.ShowLegend(Edge.Top);
    }

    private void RenderBodeStepResponse(double[] time, double[] trim, double[] angle)
    {
        var view = GetBodeView();
        if (view == null) return;

        var plot = view.PlotStepResponseControl.Plot;
        plot.Clear();

        var trimScatter = plot.Add.Scatter(time, trim, Color.FromHex("#FFA502"));
        trimScatter.LegendText = "Трим (°)";
        trimScatter.MarkerSize = 0;

        var angleScatter = plot.Add.Scatter(time, angle, Color.FromHex("#4A90FF"));
        angleScatter.LegendText = "Угол (°)";
        angleScatter.MarkerSize = 0;

        plot.Axes.AutoScale();
        view.PlotStepResponseControl.Refresh();
    }

    private void RenderBodeResult(BodeResult result)
    {
        var view = GetBodeView();
        if (view == null) return;

        double[] logFreqs = result.Loop.Select(p => Math.Log10(p.FreqHz)).ToArray();
        double[] logClosedFreqs = result.ClosedLoop.Select(p => Math.Log10(p.FreqHz)).ToArray();
        double[] loopMag = result.Loop.Select(p => p.MagDb).ToArray();
        double[] loopPhase = result.Loop.Select(p => p.PhaseDeg).ToArray();
        double[] closedMag = result.ClosedLoop.Select(p => p.MagDb).ToArray();

        double xMin = Math.Log10(BodeAnalyzer.MinFreqHz) - 0.1;
        double xMax = Math.Log10(Math.Max(10.0, result.SampleRateHz / 2.0)) + 0.1;

        // --- АЧХ ---
        var magPlot = view.PlotBodeMagControl.Plot;
        magPlot.Clear();

        var loopMagScatter = magPlot.Add.Scatter(logFreqs, loopMag, Color.FromHex("#4A90FF"));
        loopMagScatter.LegendText = "Контур L";
        loopMagScatter.MarkerSize = 0;
        loopMagScatter.LineWidth = 2;

        var closedMagScatter = magPlot.Add.Scatter(logClosedFreqs, closedMag, Color.FromHex("#78909C"));
        closedMagScatter.LegendText = "Замкнутая T";
        closedMagScatter.MarkerSize = 0;
        closedMagScatter.LinePattern = LinePattern.Dashed;

        magPlot.Add.HorizontalLine(0, 1, Color.FromHex("#2A3040"), LinePattern.Solid);

        if (result.GainCrossoverHz.HasValue)
        {
            magPlot.Add.VerticalLine(Math.Log10(result.GainCrossoverHz.Value), 1, Color.FromHex("#00D084"), LinePattern.Dashed);
        }

        magPlot.Axes.SetLimitsX(xMin, xMax);
        magPlot.Axes.AutoScaleY();
        view.PlotBodeMagControl.Refresh();

        // --- ФЧХ ---
        var phasePlot = view.PlotBodePhaseControl.Plot;
        phasePlot.Clear();

        var loopPhaseScatter = phasePlot.Add.Scatter(logFreqs, loopPhase, Color.FromHex("#00D084"));
        loopPhaseScatter.LegendText = "Фаза L";
        loopPhaseScatter.MarkerSize = 0;
        loopPhaseScatter.LineWidth = 2;

        phasePlot.Add.HorizontalLine(-180, 1, Color.FromHex("#2A3040"), LinePattern.Solid);

        if (result.PhaseCrossoverHz.HasValue)
        {
            phasePlot.Add.VerticalLine(Math.Log10(result.PhaseCrossoverHz.Value), 1, Color.FromHex("#FF4081"), LinePattern.Dashed);
        }

        phasePlot.Axes.SetLimitsX(xMin, xMax);
        phasePlot.Axes.AutoScaleY();
        view.PlotBodePhaseControl.Refresh();
    }

    private void OnWindowKeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key == Key.Space)
        {
            _vm.DisarmRobotCommand.Execute(null);
            e.Handled = true;
            return;
        }

        if (e.Key == Key.OemOpenBrackets)
        {
            _vm.Dashboard?.AdjustTrimCommand.Execute("-0.05");
            e.Handled = true;
            return;
        }
        if (e.Key == Key.OemCloseBrackets)
        {
            _vm.Dashboard?.AdjustTrimCommand.Execute("0.05");
            e.Handled = true;
            return;
        }

        if (_vm.Control?.IsKeyboardActive == true)
        {
            HandleDriveKeys(e.Key, isDown: true);
        }
    }

    private void OnWindowKeyUp(object sender, KeyEventArgs e)
    {
        if (_vm.Control?.IsKeyboardActive == true)
        {
            HandleDriveKeys(e.Key, isDown: false);
        }
    }

    private void HandleDriveKeys(Key key, bool isDown)
    {
        bool w = key == Key.W || key == Key.Up;
        bool s = key == Key.S || key == Key.Down;
        bool a = key == Key.A || key == Key.Left;
        bool d = key == Key.D || key == Key.Right;

        if (w || s || a || d)
        {
            _vm.Control?.UpdateKey(isDown, w, s, a, d);
        }
    }
}

[ValueConversion(typeof(bool), typeof(Visibility))]
public class InverseBooleanToVisibilityConverter : IValueConverter
{
    public object Convert(object value, Type targetType, object parameter, CultureInfo culture)
    {
        return value is true ? Visibility.Collapsed : Visibility.Visible;
    }

    public object ConvertBack(object value, Type targetType, object parameter, CultureInfo culture)
    {
        return value is Visibility.Visible ? false : true;
    }
}
