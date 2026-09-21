using System.Windows;
using System.Windows.Controls;
using ScottPlot.WPF;

namespace BalanceBot.Desktop.Views;

public partial class DashboardView : UserControl
{
    public WpfPlot PlotOrientationControl => PlotOrientation;
    public WpfPlot PlotMotorsControl => PlotMotors;

    private bool _isSidebarVisible = true;

    public DashboardView()
    {
        InitializeComponent();
        Loaded += OnLoaded;
        SizeChanged += OnSizeChanged;
    }

    private void OnLoaded(object sender, RoutedEventArgs e)
    {
        UpdateLayoutState();
    }

    private void OnSizeChanged(object sender, SizeChangedEventArgs e)
    {
        UpdateLayoutState();
    }

    private void UpdateLayoutState()
    {
        // Широкая раскладка требует: графики (MinWidth 400) + отступ 12 + панель 320 = 732
        double width = ActualWidth;
        if (width < 740)
        {
            VisualStateManager.GoToState(this, "Narrow", true);
            _isSidebarVisible = false;
        }
        else
        {
            VisualStateManager.GoToState(this, "Wide", true);
            _isSidebarVisible = true;
        }
    }

    private void ToggleSidebar_Click(object sender, RoutedEventArgs e)
    {
        _isSidebarVisible = !_isSidebarVisible;
        if (_isSidebarVisible)
        {
            VisualStateManager.GoToState(this, "Wide", true);
        }
        else
        {
            VisualStateManager.GoToState(this, "Narrow", true);
        }
    }
}
