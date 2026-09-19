using System.Windows.Controls;
using ScottPlot.WPF;

namespace BalanceBot.Desktop.Views;

public partial class DashboardView : UserControl
{
    public WpfPlot PlotOrientationControl => PlotOrientation;
    public WpfPlot PlotMotorsControl => PlotMotors;

    public DashboardView()
    {
        InitializeComponent();
    }
}
