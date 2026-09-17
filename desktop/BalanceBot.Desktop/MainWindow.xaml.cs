using System.Globalization;
using System.Windows;
using System.Windows.Data;
using System.Windows.Input;
using BalanceBot.Core.Analysis;
using BalanceBot.Desktop.ViewModels;
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
        SetupPlots();
        SetupBlackBoxPlotTheme();
        SetupBodePlotsTheme();
    }

    private void SetupPlots()
    {
        var pOri = PlotOrientation.Plot;
        pOri.FigureBackground.Color = Color.FromHex("#12141A");
        pOri.DataBackground.Color = Color.FromHex("#1B1F28");
        pOri.Axes.Color(Color.FromHex("#8B93A7"));
        pOri.Grid.MajorLineColor = Color.FromHex("#222733");
        pOri.Axes.Title.Label.Text = "Ориентация и IMU (Градусы °)";
        pOri.Axes.Title.Label.FontSize = 13;

        var angleStreamer = pOri.Add.DataStreamer(600);
        angleStreamer.Color = Color.FromHex("#3D7EFF");
        angleStreamer.ViewScrollLeft();

        var gyroStreamer = pOri.Add.DataStreamer(600);
        gyroStreamer.Color = Color.FromHex("#00E676");
        gyroStreamer.ViewScrollLeft();

        var trimStreamer = pOri.Add.DataStreamer(600);
        trimStreamer.Color = Color.FromHex("#E8EAF0");
        trimStreamer.ViewScrollLeft();

        var accelStreamer = pOri.Add.DataStreamer(600);
        accelStreamer.Color = Color.FromHex("#80D8FF");
        accelStreamer.IsVisible = false;
        accelStreamer.ViewScrollLeft();

        var leanStreamer = pOri.Add.DataStreamer(600);
        leanStreamer.Color = Color.FromHex("#FF6D00");
        leanStreamer.IsVisible = false;
        leanStreamer.ViewScrollLeft();

        var pMot = PlotMotors.Plot;
        pMot.FigureBackground.Color = Color.FromHex("#12141A");
        pMot.DataBackground.Color = Color.FromHex("#1B1F28");
        pMot.Axes.Color(Color.FromHex("#8B93A7"));
        pMot.Grid.MajorLineColor = Color.FromHex("#222733");
        pMot.Axes.Title.Label.Text = "Привод колес и регулятор (об/с)";
        pMot.Axes.Title.Label.FontSize = 13;

        var cmdStreamer = pMot.Add.DataStreamer(600);
        cmdStreamer.Color = Color.FromHex("#FFB300");
        cmdStreamer.ViewScrollLeft();

        var wheelStreamer = pMot.Add.DataStreamer(600);
        wheelStreamer.Color = Color.FromHex("#B388FF");
        wheelStreamer.ViewScrollLeft();

        var termPStreamer = pMot.Add.DataStreamer(600);
        termPStreamer.Color = Color.FromHex("#FF4081");
        termPStreamer.IsVisible = false;
        termPStreamer.ViewScrollLeft();

        var termDStreamer = pMot.Add.DataStreamer(600);
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
                    PlotOrientation.Refresh();
                    PlotMotors.Refresh();
                };

                d.RequestPlotsReset = () =>
                {
                    PlotOrientation.Plot.Axes.AutoScale();
                    PlotMotors.Plot.Axes.AutoScale();
                    PlotOrientation.Refresh();
                    PlotMotors.Refresh();
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

        PlotOrientation.Refresh();
        PlotMotors.Refresh();
    }

    private void SetupBlackBoxPlotTheme()
    {
        var plot = PlotBlackBox.Plot;
        plot.FigureBackground.Color = Color.FromHex("#12141A");
        plot.DataBackground.Color = Color.FromHex("#1B1F28");
        plot.Axes.Color(Color.FromHex("#8B93A7"));
        plot.Grid.MajorLineColor = Color.FromHex("#222733");
        // Легенда добавляется один раз: ShowLegend(Edge) создает новую панель при каждом вызове
        plot.ShowLegend(Edge.Top);
        PlotBlackBox.Refresh();
    }

    private void RenderBlackBoxFrames(List<BlackBoxFrameDto> frames)
    {
        var plot = PlotBlackBox.Plot;
        plot.Clear();

        uint startMs = frames[0].TimestampMs;
        double[] times = frames.Select(f => (double)(f.TimestampMs - startMs)).ToArray();
        double[] angles = frames.Select(f => (double)f.Angle).ToArray();
        double[] gyros = frames.Select(f => (double)f.Gyro).ToArray();
        double[] cmds = frames.Select(f => (double)f.Cmd / 3200.0).ToArray();

        var angleScatter = plot.Add.Scatter(times, angles, Color.FromHex("#3D7EFF"));
        angleScatter.LegendText = "Угол наклона (°)";

        var gyroScatter = plot.Add.Scatter(times, gyros, Color.FromHex("#00E676"));
        gyroScatter.LegendText = "Гироскоп (°/с)";

        var cmdScatter = plot.Add.Scatter(times, cmds, Color.FromHex("#FFB300"));
        cmdScatter.LegendText = "Команда мотора (об/с)";

        plot.Axes.AutoScale();
        PlotBlackBox.Refresh();
    }

    private void SetupBodePlotsTheme()
    {
        foreach (var wpfPlot in new[] { PlotStepResponse, PlotBodeMag, PlotBodePhase })
        {
            var plot = wpfPlot.Plot;
            plot.FigureBackground.Color = Color.FromHex("#12141A");
            plot.DataBackground.Color = Color.FromHex("#1B1F28");
            plot.Axes.Color(Color.FromHex("#8B93A7"));
            plot.Grid.MajorLineColor = Color.FromHex("#222733");
            wpfPlot.Refresh();
        }

        PlotStepResponse.Plot.Axes.Title.Label.Text = "Отклик на импульс трима";
        PlotStepResponse.Plot.Axes.Title.Label.FontSize = 13;

        PlotBodeMag.Plot.Axes.Title.Label.Text = "АЧХ контура |L| (дБ), ось X — log10(f/Гц)";
        PlotBodeMag.Plot.Axes.Title.Label.FontSize = 13;

        PlotBodePhase.Plot.Axes.Title.Label.Text = "ФЧХ контура ∠L (°), ось X — log10(f/Гц)";
        PlotBodePhase.Plot.Axes.Title.Label.FontSize = 13;

        // Легенды добавляются один раз: ShowLegend(Edge) создает новую панель при каждом вызове
        PlotStepResponse.Plot.ShowLegend(Edge.Top);
        PlotBodeMag.Plot.ShowLegend(Edge.Top);
        PlotBodePhase.Plot.ShowLegend(Edge.Top);
    }

    private void RenderBodeStepResponse(double[] time, double[] trim, double[] angle)
    {
        var plot = PlotStepResponse.Plot;
        plot.Clear();

        var trimScatter = plot.Add.Scatter(time, trim, Color.FromHex("#FFB300"));
        trimScatter.LegendText = "Трим (°)";
        trimScatter.MarkerSize = 0;

        var angleScatter = plot.Add.Scatter(time, angle, Color.FromHex("#3D7EFF"));
        angleScatter.LegendText = "Угол (°)";
        angleScatter.MarkerSize = 0;

        plot.Axes.AutoScale();
        PlotStepResponse.Refresh();
    }

    private void RenderBodeResult(BodeResult result)
    {
        // Ось X в логарифмическом масштабе через log10(f): ScottPlot 5.0 не имеет встроенной лог-оси
        double[] logFreqs = result.Loop.Select(p => Math.Log10(p.FreqHz)).ToArray();
        double[] logClosedFreqs = result.ClosedLoop.Select(p => Math.Log10(p.FreqHz)).ToArray();
        double[] loopMag = result.Loop.Select(p => p.MagDb).ToArray();
        double[] loopPhase = result.Loop.Select(p => p.PhaseDeg).ToArray();
        double[] closedMag = result.ClosedLoop.Select(p => p.MagDb).ToArray();

        double xMin = Math.Log10(BodeAnalyzer.MinFreqHz) - 0.1;
        double xMax = Math.Log10(Math.Max(10.0, result.SampleRateHz / 2.0)) + 0.1;

        // --- АЧХ ---
        var magPlot = PlotBodeMag.Plot;
        magPlot.Clear();

        var loopMagScatter = magPlot.Add.Scatter(logFreqs, loopMag, Color.FromHex("#3D7EFF"));
        loopMagScatter.LegendText = "Контур L";
        loopMagScatter.MarkerSize = 0;
        loopMagScatter.LineWidth = 2;

        var closedMagScatter = magPlot.Add.Scatter(logClosedFreqs, closedMag, Color.FromHex("#78909C"));
        closedMagScatter.LegendText = "Замкнутая T";
        closedMagScatter.MarkerSize = 0;
        closedMagScatter.LinePattern = LinePattern.Dashed;

        magPlot.Add.HorizontalLine(0, 1, Color.FromHex("#39445C"), LinePattern.Solid);

        if (result.GainCrossoverHz.HasValue)
        {
            magPlot.Add.VerticalLine(Math.Log10(result.GainCrossoverHz.Value), 1, Color.FromHex("#00E676"), LinePattern.Dashed);
        }

        magPlot.Axes.SetLimitsX(xMin, xMax);
        magPlot.Axes.AutoScaleY();
        PlotBodeMag.Refresh();

        // --- ФЧХ ---
        var phasePlot = PlotBodePhase.Plot;
        phasePlot.Clear();

        var loopPhaseScatter = phasePlot.Add.Scatter(logFreqs, loopPhase, Color.FromHex("#00E676"));
        loopPhaseScatter.LegendText = "Фаза L";
        loopPhaseScatter.MarkerSize = 0;
        loopPhaseScatter.LineWidth = 2;

        phasePlot.Add.HorizontalLine(-180, 1, Color.FromHex("#39445C"), LinePattern.Solid);

        if (result.PhaseCrossoverHz.HasValue)
        {
            phasePlot.Add.VerticalLine(Math.Log10(result.PhaseCrossoverHz.Value), 1, Color.FromHex("#FF4081"), LinePattern.Dashed);
        }

        phasePlot.Axes.SetLimitsX(xMin, xMax);
        phasePlot.Axes.AutoScaleY();
        PlotBodePhase.Refresh();
    }

    private void OnWindowKeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key == Key.Space)
        {
            _vm.DisarmRobotCommand.Execute(null);
            e.Handled = true;
            return;
        }

        // Горячие клавиши микротримминга угла на лету: [ и ]
        if (e.Key == Key.OemOpenBrackets) // Клавиша [ (или русская Х) -> чуть назад
        {
            _vm.Dashboard?.AdjustTrimCommand.Execute("-0.05");
            e.Handled = true;
            return;
        }
        if (e.Key == Key.OemCloseBrackets) // Клавиша ] (или русская Ъ) -> чуть вперед
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