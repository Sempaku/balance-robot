using System.Windows.Controls;
using ScottPlot.WPF;

namespace BalanceBot.Desktop.Views;

public partial class BodeView : UserControl
{
    public WpfPlot PlotStepResponseControl => PlotStepResponse;
    public WpfPlot PlotBodeMagControl => PlotBodeMag;
    public WpfPlot PlotBodePhaseControl => PlotBodePhase;

    public BodeView()
    {
        InitializeComponent();
    }
}
